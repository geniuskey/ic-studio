"""Generate the animated SVGs of the guide's "Learn IC design" chapters (docs/images/learn-*.svg).

The inverter transfer curve is drawn from a real simulation:

    ics spice inv.cir --json > inv.json      # the generic inverter with `.dc VIN 0 1.8 10m`
    python3 scripts/learn-svgs.py docs/images inv.json
"""

import json
import math
import sys
from pathlib import Path

OUT = Path(sys.argv[1])
INV = Path(sys.argv[2])  # JSON of the inverter DC sweep (ics spice --json)

STYLE_BASE = """
    text { font-family: Inter, 'Segoe UI', Helvetica, Arial, sans-serif; fill: #8a90b0; font-size: 12px; }
    .mono { font-family: 'JetBrains Mono', Consolas, Menlo, monospace; }
    .hi { fill: #e6e8f2; }
    .title { fill: #e6e8f2; font-size: 14px; font-weight: 600; }
    .grid { stroke: #262c48; stroke-width: 1; fill: none; }
    .axis { stroke: #3a4066; stroke-width: 1.5; fill: none; }
    .wire { stroke: #c9cde6; stroke-width: 2; fill: none; stroke-linecap: round; stroke-linejoin: round; }
    .a { animation-iteration-count: infinite; animation-fill-mode: both; }
    @media (prefers-reduced-motion: reduce) { * { animation: none !important; } }
"""


def svg(name, w, h, label, style, body):
    s = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img" aria-label="{label}">
  <style>{STYLE_BASE}{style}
  </style>
  <rect x="0.5" y="0.5" width="{w - 1}" height="{h - 1}" rx="14" fill="#0f1220" stroke="#262c48"/>
{body}
</svg>
"""
    (OUT / name).write_text(s)


def path(points):
    return "M" + " L".join(f"{x:.1f} {y:.1f}" for x, y in points)


# ---------------------------------------------------------------- chip zoom
def chip_zoom():
    panels = []
    cx = [110, 300, 490, 680]
    labels = [
        ("Wafer", "300 mm, hundreds of chips"),
        ("Chip (die)", "a few mm, blocks and wiring"),
        ("Cell", "a few µm, a handful of transistors"),
        ("Transistor", "180 nm gate, the switch"),
    ]
    # wafer
    wafer = ['<circle cx="0" cy="0" r="62" fill="#1b2140" stroke="#5a6190" stroke-width="1.5"/>']
    for i in range(-3, 4):
        for j in range(-3, 4):
            x, y = i * 16 - 7, j * 16 - 7
            if (abs(x) + 7) ** 2 + (abs(y) + 7) ** 2 < 58**2 and (abs(x + 14)) ** 2 + (abs(y + 14)) ** 2 < 58**2:
                wafer.append(f'<rect x="{x}" y="{y}" width="14" height="14" rx="1.5" fill="#2b3360" stroke="#3f4980"/>')
    wafer.append('<rect class="zoombox" x="9" y="-7" width="14" height="14" fill="none" stroke="#f5b83d" stroke-width="1.6"/>')
    # die
    die = [
        '<rect x="-60" y="-60" width="120" height="120" rx="4" fill="#1b2140" stroke="#5a6190" stroke-width="1.5"/>',
        '<rect x="-50" y="-50" width="46" height="36" fill="#2b3360" stroke="#3f4980"/>',
        '<rect x="4" y="-50" width="46" height="56" fill="#26304f" stroke="#3f4980"/>',
        '<rect x="-50" y="-6" width="46" height="56" fill="#26304f" stroke="#3f4980"/>',
        '<rect x="4" y="14" width="46" height="36" fill="#2b3360" stroke="#3f4980"/>',
    ]
    for k in range(-48, 50, 6):
        die.append(f'<path d="M{k} -12 V-8" stroke="#508cfa" stroke-width="1"/>')
    for k in range(-48, 50, 8):
        die.append(f'<path d="M-2 {k} H2" stroke="#d264e6" stroke-width="1"/>')
    die.append('<rect class="zoombox" x="12" y="20" width="16" height="16" fill="none" stroke="#f5b83d" stroke-width="1.6"/>')
    # cell: tiny inverter layout
    cell = [
        '<rect x="-60" y="-60" width="120" height="120" rx="4" fill="#161a2c" stroke="#5a6190" stroke-width="1.5"/>',
        '<rect x="-52" y="-56" width="104" height="52" fill="rgba(120,200,120,0.18)" stroke="#78c878" stroke-dasharray="3 2"/>',
        '<rect x="-34" y="-44" width="68" height="26" fill="rgba(70,190,90,0.45)" stroke="#46be5a"/>',
        '<rect x="-34" y="18" width="68" height="20" fill="rgba(70,190,90,0.45)" stroke="#46be5a"/>',
        '<rect x="-4" y="-52" width="8" height="98" fill="rgba(230,60,60,0.65)" stroke="#e63c3c"/>',
        '<rect x="-30" y="-58" width="16" height="116" fill="rgba(80,140,250,0.4)" stroke="#508cfa"/>',
        '<rect x="14" y="-36" width="16" height="70" fill="rgba(80,140,250,0.4)" stroke="#508cfa"/>',
    ]
    for (x, y) in [(-26, -36), (-26, 24), (18, -36), (18, 24)]:
        cell.append(f'<rect x="{x}" y="{y}" width="8" height="8" fill="#ebebeb"/>')
    cell.append('<rect class="zoombox" x="-14" y="12" width="28" height="32" fill="none" stroke="#f5b83d" stroke-width="1.6"/>')
    # transistor cross-section
    tr = [
        '<rect x="-62" y="-10" width="124" height="62" fill="#2a2f4a" stroke="#5a6190" stroke-width="1.5"/>',
        '<text x="0" y="44" text-anchor="middle" style="font-size:10px">p-substrate</text>',
        '<path d="M-56 -10 V6 Q-56 14 -48 14 H-20 Q-14 14 -14 6 V-10 Z" fill="rgba(230,120,60,0.6)" stroke="#e6783c"/>',
        '<path d="M14 -10 V6 Q14 14 20 14 H48 Q56 14 56 6 V-10 Z" fill="rgba(230,120,60,0.6)" stroke="#e6783c"/>',
        '<text x="-35" y="6" text-anchor="middle" class="hi" style="font-size:10px">n+</text>',
        '<text x="35" y="6" text-anchor="middle" class="hi" style="font-size:10px">n+</text>',
        '<rect x="-16" y="-14" width="32" height="4" fill="#9aa3d0"/>',
        '<rect x="-16" y="-34" width="32" height="20" fill="rgba(230,60,60,0.7)" stroke="#e63c3c"/>',
        '<rect class="chan" x="-14" y="-10" width="28" height="3" fill="#5cc0ff"/>',
        '<text x="-35" y="-18" text-anchor="middle" style="font-size:10px">S</text>',
        '<text x="0" y="-40" text-anchor="middle" style="font-size:10px">G</text>',
        '<text x="35" y="-18" text-anchor="middle" style="font-size:10px">D</text>',
    ]
    groups = [wafer, die, cell, tr]
    for i, (g, x) in enumerate(zip(groups, cx)):
        inner = "\n      ".join(g)
        t, sub = labels[i]
        panels.append(f"""  <g class="s{i + 1}">
    <rect class="card a" x="{x - 82}" y="38" width="164" height="190" rx="12"/>
    <g transform="translate({x} 128)">
      {inner}
    </g>
    <text x="{x}" y="254" text-anchor="middle" class="title">{t}</text>
    <text x="{x}" y="274" text-anchor="middle">{sub}</text>
  </g>""")
    arrows = "".join(
        f'<path d="M{x + 88} 128 h14" stroke="#5a6190" stroke-width="2" fill="none"/>'
        f'<path d="M{x + 98} 122 l6 6 l-6 6" stroke="#5a6190" stroke-width="2" fill="none"/>'
        for x in cx[:3]
    )
    style = """
    .card { fill: #161a2c; stroke: #3a4066; stroke-width: 1.5; animation-name: lit; animation-duration: 8s; }
    .s1 .card { animation-delay: 0s; } .s2 .card { animation-delay: 2s; }
    .s3 .card { animation-delay: 4s; } .s4 .card { animation-delay: 6s; }
    @keyframes lit { 0% { stroke: #3a4066; fill: #161a2c; } 3%, 22% { stroke: #9d90ff; fill: #1f2442; } 26%, 100% { stroke: #3a4066; fill: #161a2c; } }
    .zoombox { animation: blink 2s infinite; }
    @keyframes blink { 0%, 100% { opacity: 1; } 50% { opacity: 0.3; } }
    .chan { animation: chan 2s infinite; }
    @keyframes chan { 0%, 30% { opacity: 0; } 50%, 85% { opacity: 1; } 100% { opacity: 0; } }
"""
    body = "\n".join(panels) + "\n  " + arrows
    svg("learn-chip-zoom.svg", 800, 300,
        "Zooming from a wafer to a chip, to a cell, to a single transistor", style, body)


# ---------------------------------------------------------------- RC charging
def rc_charge():
    X0, X1, Y0, Y1 = 380, 760, 60, 280  # plot box
    T = 5.0  # µs
    def px(t): return X0 + (X1 - X0) * t / T
    def py(v): return Y1 - (Y1 - Y0) * v / 1.0
    t_on = 0.5
    vin = [(px(0), py(0)), (px(t_on), py(0)), (px(t_on), py(1)), (px(T), py(1))]
    vout = [(px(0), py(0)), (px(t_on), py(0))]
    for i in range(1, 201):
        t = t_on + (T - t_on) * i / 200
        vout.append((px(t), py(1 - math.exp(-(t - t_on)))))
    tau_x, tau_y = px(t_on + 1), py(1 - math.exp(-1))
    style = """
    .tin { fill: none; stroke: #5cc0ff; stroke-width: 2.2; }
    .tout { fill: none; stroke: #9d90ff; stroke-width: 2.4; }
    .cover { animation-name: sweep; animation-duration: 8s; animation-timing-function: linear; }
    @keyframes sweep { 0% { transform: translateX(-380px); } 60%, 100% { transform: translateX(0); } }
    .mark { animation-name: mark; animation-duration: 8s; }
    @keyframes mark { 0%, 24% { opacity: 0; } 30%, 94% { opacity: 1; } 99%, 100% { opacity: 0; } }
    .charge { animation-name: charge; animation-duration: 8s; animation-timing-function: linear; }
    @keyframes charge { 0%, 6% { transform: scaleY(0); } 15% { transform: scaleY(0.63); } 30% { transform: scaleY(0.86); } 60%, 100% { transform: scaleY(1); } }
"""
    body = f"""  <rect x="24" y="20" width="320" height="300" rx="10" fill="#161a2c"/>
  <text x="44" y="46" class="title">RC low-pass, R = 1 kΩ, C = 1 nF</text>
  <!-- source -->
  <circle cx="80" cy="180" r="22" fill="none" stroke="#5cc0ff" stroke-width="2"/>
  <path d="M70 186 h6 v-12 h8 v12 h6" stroke="#5cc0ff" stroke-width="1.8" fill="none"/>
  <text x="80" y="226" text-anchor="middle" class="hi">in</text>
  <path class="wire" d="M80 158 V100 H130"/>
  <path class="wire" d="M130 100 l6 -9 l10 18 l10 -18 l10 18 l10 -18 l10 18 l6 -9"/>
  <text x="166" y="84" text-anchor="middle" class="hi">R</text>
  <path class="wire" d="M192 100 H260 V150"/>
  <circle cx="260" cy="100" r="3.5" fill="#9d90ff"/>
  <text x="270" y="94" class="hi">out</text>
  <path class="wire" d="M240 150 H280 M240 162 H280"/>
  <text x="290" y="160" class="hi">C</text>
  <g transform="translate(244 160)"><rect class="charge a" x="0" y="-60" width="0" height="0"/></g>
  <path class="wire" d="M260 162 V250 M80 202 V250 H260"/>
  <path class="wire" d="M150 250 V262 M138 262 H162 M143 268 H157 M148 274 H152"/>
  <text x="44" y="300">τ = R · C = 1 µs,  f(-3 dB) = 1 / (2π R C) ≈ 159 kHz</text>

  <rect x="354" y="20" width="422" height="300" rx="10" fill="#161a2c"/>
  <text x="{X0}" y="46" class="title">Step response</text>
  <path class="tin" d="M640 42 H664"/><text x="670" y="46" class="hi">in</text>
  <path class="tout" d="M700 42 H724"/><text x="730" y="46" class="hi">out</text>
  <defs><clipPath id="plot"><rect x="{X0}" y="{Y0 - 4}" width="{X1 - X0}" height="{Y1 - Y0 + 8}"/></clipPath></defs>
  <path class="grid" d="M{X0} {py(0.5):.0f} H{X1} M{X0} {py(1):.0f} H{X1}" stroke-dasharray="3 5"/>
  <g clip-path="url(#plot)">
    <path class="tin" d="{path(vin)}"/>
    <path class="tout" d="{path(vout)}"/>
    <rect class="cover a" x="{X1}" y="{Y0 - 4}" width="400" height="{Y1 - Y0 + 8}" fill="#161a2c"/>
  </g>
  <path class="axis" d="M{X0} {Y0} V{Y1} H{X1}"/>
  <g text-anchor="middle">
    {"".join(f'<text x="{px(t):.0f}" y="{Y1 + 18}">{t:g}</text>' for t in range(0, 6))}
    <text x="{(X0 + X1) / 2:.0f}" y="{Y1 + 34}" class="hi">time (µs)</text>
  </g>
  <g text-anchor="end"><text x="{X0 - 6}" y="{py(0) + 4:.0f}">0</text><text x="{X0 - 6}" y="{py(1) + 4:.0f}">1 V</text></g>
  <g class="mark a">
    <path d="M{tau_x:.1f} {Y1} V{tau_y:.1f} H{X0}" stroke="#f5b83d" stroke-width="1.2" stroke-dasharray="4 3" fill="none"/>
    <circle cx="{tau_x:.1f}" cy="{tau_y:.1f}" r="4" fill="#f5b83d"/>
    <text x="{tau_x + 8:.1f}" y="{tau_y + 18:.1f}" style="fill:#f5b83d">63 % after one τ</text>
  </g>"""
    # the capacitor "charge" bar: use a separate simple rect that scales
    body = body.replace(
        '<g transform="translate(244 160)"><rect class="charge a" x="0" y="-60" width="0" height="0"/></g>',
        '<g transform="translate(318 170)"><rect x="0" y="-60" width="10" height="60" rx="2" fill="none" stroke="#3a4066"/>'
        '<rect class="charge a" x="0" y="-60" width="10" height="60" rx="2" fill="#9d90ff" '
        'style="transform-box: fill-box; transform-origin: 50% 100%"/></g>',
    )
    svg("learn-rc-charge.svg", 800, 340,
        "An RC low-pass: a voltage step at the input charges the capacitor, the output rises exponentially and reaches 63 percent after one time constant",
        style, body)


# ---------------------------------------------------------------- MOSFET channel
def mosfet_channel():
    style = """
    .gauge { animation-name: gauge; animation-duration: 9s; animation-timing-function: ease-in-out; transform-box: fill-box; transform-origin: 50% 100%; }
    @keyframes gauge { 0% { transform: scaleY(0.02); } 35%, 68% { transform: scaleY(1); } 88%, 100% { transform: scaleY(0.02); } }
    .gate { animation-name: gate; animation-duration: 9s; }
    @keyframes gate { 0% { fill: rgba(230,60,60,0.35); } 35%, 68% { fill: rgba(230,60,60,0.9); } 88%, 100% { fill: rgba(230,60,60,0.35); } }
    .dep { animation-name: dep; animation-duration: 9s; }
    @keyframes dep { 0%, 3% { opacity: 0; } 10%, 80% { opacity: 1; } 86%, 100% { opacity: 0; } }
    .chan, .on { animation-name: chan; animation-duration: 9s; }
    @keyframes chan { 0%, 9% { opacity: 0; } 16%, 76% { opacity: 1; } 82%, 100% { opacity: 0; } }
    .off { animation-name: off; animation-duration: 9s; }
    @keyframes off { 0%, 9% { opacity: 1; } 14%, 78% { opacity: 0; } 84%, 100% { opacity: 1; } }
    .e { animation: flow 1.6s linear infinite; }
    @keyframes flow { from { transform: translateX(0); } to { transform: translateX(220px); } }
    .plus { fill: #e6e8f2; font-size: 13px; font-weight: 700; }
"""
    electrons = "".join(
        f'<circle class="e" cx="{120 + i * 22}" cy="196" r="3" fill="#e6e8f2"/>'
        for i in range(24)
    )
    body = f"""  <!-- gauge -->
  <rect x="24" y="20" width="150" height="320" rx="10" fill="#161a2c"/>
  <text x="99" y="46" text-anchor="middle" class="title">V<tspan dy="4" style="font-size:10px">GS</tspan></text>
  <rect x="80" y="70" width="38" height="220" rx="6" fill="#0f1220" stroke="#3a4066"/>
  <rect class="gauge a" x="84" y="74" width="30" height="212" rx="4" fill="#9d90ff"/>
  <path d="M74 233 H124" stroke="#f5b83d" stroke-width="1.5" stroke-dasharray="4 3"/>
  <text x="128" y="237" style="fill:#f5b83d">V<tspan dy="3" style="font-size:9px">th</tspan></text>
  <text x="70" y="78" text-anchor="end">1.8 V</text>
  <text x="70" y="290" text-anchor="end">0</text>
  <text x="99" y="316" text-anchor="middle">gate voltage</text>

  <!-- cross-section -->
  <rect x="190" y="20" width="586" height="320" rx="10" fill="#161a2c"/>
  <text x="210" y="46" class="title">NMOS cross-section</text>
  <rect x="210" y="190" width="546" height="130" fill="#2a2f4a" stroke="#5a6190"/>
  <text x="483" y="306" text-anchor="middle">p-type substrate (holes are the majority carriers)</text>
  <path d="M220 190 V232 Q220 252 240 252 H330 Q350 252 350 232 V190 Z" fill="rgba(230,120,60,0.55)" stroke="#e6783c"/>
  <path d="M616 190 V232 Q616 252 636 252 H726 Q746 252 746 232 V190 Z" fill="rgba(230,120,60,0.55)" stroke="#e6783c"/>
  <text x="285" y="236" text-anchor="middle" class="hi">n+ source</text>
  <text x="681" y="236" text-anchor="middle" class="hi">n+ drain</text>
  <path class="dep a" d="M350 192 H616 V236 Q483 250 350 236 Z" fill="rgba(245,184,61,0.10)" stroke="#f5b83d" stroke-dasharray="4 3"/>
  <rect x="346" y="178" width="274" height="12" fill="#9aa3d0"/>
  <text x="626" y="188" style="font-size:10px">thin oxide</text>
  <rect class="gate a" x="346" y="120" width="274" height="58" stroke="#e63c3c"/>
  <text x="483" y="154" text-anchor="middle" class="hi">poly gate</text>
  <g class="gate-plus on a"><text class="plus" x="370" y="138">+</text><text class="plus" x="430" y="138">+</text><text class="plus" x="490" y="138">+</text><text class="plus" x="550" y="138">+</text><text class="plus" x="600" y="138">+</text></g>
  <path class="wire" d="M285 190 V80 M483 120 V80 M681 190 V80"/>
  <text x="285" y="72" text-anchor="middle" class="hi">S (0 V)</text>
  <text x="483" y="72" text-anchor="middle" class="hi">G</text>
  <text x="681" y="72" text-anchor="middle" class="hi">D (+)</text>
  <g class="chan a">
    <rect x="340" y="191" width="286" height="9" fill="rgba(92,192,255,0.55)"/>
    <defs><clipPath id="ch"><rect x="342" y="190" width="282" height="12"/></clipPath></defs>
    <g clip-path="url(#ch)">{electrons}</g>
    <path d="M520 268 H600" stroke="#5cc0ff" stroke-width="2"/><path d="M594 262 l8 6 l-8 6" stroke="#5cc0ff" stroke-width="2" fill="none"/>
    <text x="510" y="272" text-anchor="end" style="fill:#5cc0ff">electrons flow S → D, current I<tspan dy="3" style="font-size:9px">D</tspan></text>
  </g>
  <text class="off a" x="756" y="46" text-anchor="end" style="fill:#e6e8f2">V<tspan dy="3" style="font-size:9px">GS</tspan><tspan dy="-3"> &lt; V</tspan><tspan dy="3" style="font-size:9px">th</tspan><tspan dy="-3">: no channel, the switch is OFF</tspan></text>
  <text class="on a" x="756" y="46" text-anchor="end" style="fill:#5cc0ff">V<tspan dy="3" style="font-size:9px">GS</tspan><tspan dy="-3"> &gt; V</tspan><tspan dy="3" style="font-size:9px">th</tspan><tspan dy="-3">: an n-type channel forms, the switch is ON</tspan></text>"""
    svg("learn-mosfet-channel.svg", 800, 360,
        "NMOS cross-section: as the gate voltage rises above the threshold voltage an inversion channel forms under the gate and electrons flow from source to drain",
        style, body)


# ---------------------------------------------------------------- MOSFET I-V
def mosfet_iv():
    k = 280e-6 * 1.0 / 0.18
    vth, lam = 0.45, 0.08
    X0, X1, Y0, Y1 = 90, 540, 50, 320
    IMAX = 1.8e-3
    def px(v): return X0 + (X1 - X0) * v / 1.8
    def py(i): return Y1 - (Y1 - Y0) * i / IMAX
    def ids(vgs, vds):
        vov = vgs - vth
        if vov <= 0:
            return 0
        if vds < vov:
            return k * (vov * vds - vds * vds / 2) * (1 + lam * vds)
        return 0.5 * k * vov * vov * (1 + lam * vds)
    vgss = [0.8, 1.0, 1.2, 1.4, 1.6, 1.8]
    colors = ["#4f5fa8", "#5f6fc0", "#7280d8", "#8a90ff", "#a99bff", "#c8b8ff"]
    curves = []
    for n, (vgs, c) in enumerate(zip(vgss, colors)):
        pts = [(px(v), py(ids(vgs, v))) for v in [1.8 * i / 120 for i in range(121)]]
        curves.append(
            f'<path class="cv a" d="{path(pts)}" stroke="{c}" pathLength="1" style="animation-delay:{n * 0.7:.1f}s"/>'
            f'<text class="lb a" x="{X1 + 8}" y="{py(ids(vgs, 1.8)) + 4:.1f}" style="fill:{c};animation-delay:{n * 0.7:.1f}s">{vgs:.1f} V</text>'
        )
    bnd = [(px(v), py(0.5 * k * v * v * (1 + lam * v))) for v in [1.4 * i / 60 for i in range(61)]]
    style = """
    .cv { fill: none; stroke-width: 2.4; stroke-dasharray: 1 1; animation-name: draw; animation-duration: 9s; }
    @keyframes draw { 0% { stroke-dashoffset: 1; opacity: 1; } 14%, 85% { stroke-dashoffset: 0; opacity: 1; } 92%, 100% { stroke-dashoffset: 0; opacity: 0; } }
    .lb { animation-name: lb; animation-duration: 9s; }
    @keyframes lb { 0%, 12% { opacity: 0; } 15%, 85% { opacity: 1; } 92%, 100% { opacity: 0; } }
    .bnd { fill: none; stroke: #f5b83d; stroke-width: 1.4; stroke-dasharray: 5 4; }
"""
    body = f"""  <rect x="24" y="20" width="752" height="340" rx="10" fill="#161a2c"/>
  <text x="{X0}" y="40" class="title">NMOS output curves, W/L = 1 µm / 180 nm (generic 180 nm model)</text>
  <path class="grid" d="{''.join(f'M{X0} {py(i * 1e-3 * 0.3):.0f} H{X1} ' for i in range(1, 7))}" stroke-dasharray="3 5"/>
  <path class="bnd" d="{path(bnd)}"/>
  <text x="{px(0.28):.0f}" y="{py(1.55e-3):.0f}" style="fill:#f5b83d">triode</text>
  <text x="{px(0.28):.0f}" y="{py(1.55e-3) + 14:.0f}" style="font-size:11px">(a resistor)</text>
  <text x="{px(1.4):.0f}" y="{py(1.0e-3):.0f}" style="fill:#f5b83d">saturation</text>
  <text x="{px(1.4):.0f}" y="{py(1.0e-3) + 14:.0f}" style="font-size:11px">(a current source)</text>
  <text x="{px(1.42) + 4:.0f}" y="{py(1.68e-3):.0f}" text-anchor="end" style="fill:#f5b83d;font-size:11px">V<tspan dy="3" style="font-size:8px">DS</tspan><tspan dy="-3"> = V</tspan><tspan dy="3" style="font-size:8px">GS</tspan><tspan dy="-3"> − V</tspan><tspan dy="3" style="font-size:8px">th</tspan></text>
  {''.join(curves)}
  <path class="axis" d="M{X0} {Y0} V{Y1} H{X1}"/>
  <g text-anchor="middle">{''.join(f'<text x="{px(v):.0f}" y="{Y1 + 18}">{v:g}</text>' for v in [0, 0.3, 0.6, 0.9, 1.2, 1.5, 1.8])}
    <text x="{(X0 + X1) / 2:.0f}" y="{Y1 + 34}" class="hi">V<tspan dy="3" style="font-size:9px">DS</tspan><tspan dy="-3"> (V)</tspan></text></g>
  <g text-anchor="end">{''.join(f'<text x="{X0 - 6}" y="{py(i * 0.3e-3) + 4:.0f}">{i * 0.3:.1f}</text>' for i in range(0, 7))}</g>
  <text x="36" y="{(Y0 + Y1) / 2:.0f}" class="hi" transform="rotate(-90 36 {(Y0 + Y1) / 2:.0f})" text-anchor="middle">I<tspan dy="3" style="font-size:9px">D</tspan><tspan dy="-3"> (mA)</tspan></text>
  <text x="{X1 + 8}" y="{Y0 + 2}" class="hi">V<tspan dy="3" style="font-size:9px">GS</tspan></text>
  <g transform="translate(612 120)">
    <text x="0" y="0" class="hi">Saturation:</text>
    <text x="0" y="22" class="mono hi" style="font-size:12px">I = ½·k'·(W/L)·V<tspan dy="3" style="font-size:9px">ov</tspan><tspan dy="-3">²</tspan></text>
    <text x="0" y="44">V<tspan dy="3" style="font-size:9px">ov</tspan><tspan dy="-3"> = V</tspan><tspan dy="3" style="font-size:9px">GS</tspan><tspan dy="-3"> − V</tspan><tspan dy="3" style="font-size:9px">th</tspan></text>
    <text x="0" y="66">k' = 280 µA/V²</text>
    <text x="0" y="86">V<tspan dy="3" style="font-size:9px">th</tspan><tspan dy="-3"> = 0.45 V</tspan></text>
    <text x="0" y="106">λ = 0.08 /V</text>
  </g>"""
    svg("learn-mosfet-iv.svg", 800, 380,
        "Drain current against drain voltage of an NMOS for gate voltages from 0.8 to 1.8 volts: linear triode region at low drain voltage, flat saturation region beyond the overdrive voltage",
        style, body)


# ---------------------------------------------------------------- CMOS inverter
def cmos_inverter():
    d = json.loads(INV.read_text())
    dc = [x for x in d["datasets"] if x["analysis"] == "dc"][0]["vectors"]
    vin, vout = dc["vin"], dc["v(out)"]
    X0, X1, Y0, Y1 = 470, 740, 60, 300
    def px(v): return X0 + (X1 - X0) * v / 1.8
    def py(v): return Y1 - (Y1 - Y0) * v / 1.8
    vtc = path([(px(a), py(b)) for a, b in zip(vin, vout)])
    style = """
    .s0, .s1 { animation-duration: 8s; }
    .s0 { animation-name: s0; } .s1 { animation-name: s1; }
    @keyframes s0 { 0%, 38% { opacity: 1; } 44%, 92% { opacity: 0; } 98%, 100% { opacity: 1; } }
    @keyframes s1 { 0%, 46% { opacity: 0; } 52%, 86% { opacity: 1; } 92%, 100% { opacity: 0; } }
    .vtc { fill: none; stroke: #9d90ff; stroke-width: 2.6; }
    .on { stroke: #46d08a; stroke-width: 3; fill: none; }
    .cur { stroke: #46d08a; stroke-width: 2; stroke-dasharray: 5 6; fill: none; animation: flow 0.8s linear infinite; }
    @keyframes flow { to { stroke-dashoffset: -11; } }
    .big { font-size: 22px; font-weight: 700; }
"""
    def mos(x, y, pmos):
        # gate on the left at x, channel at x+20; drain/source leads
        bubble = f'<circle cx="{x + 12}" cy="{y}" r="4" fill="none" stroke="#c9cde6" stroke-width="2"/>' if pmos else ""
        gx = x + 16
        return (f'<path class="wire" d="M{x - 30} {y} H{x + (8 if pmos else 16)}"/>{bubble}'
                f'<path class="wire" d="M{gx} {y - 18} V{y + 18} M{gx + 8} {y - 20} V{y + 20}"/>'
                f'<path class="wire" d="M{gx + 8} {y - 14} H{gx + 30} V{y - 40} M{gx + 8} {y + 14} H{gx + 30} V{y + 40}"/>')
    # PMOS at y=110, NMOS at y=230, channel line at x=?; drains meet at node y=170
    body = f"""  <rect x="24" y="20" width="390" height="320" rx="10" fill="#161a2c"/>
  <text x="44" y="46" class="title">CMOS inverter: two complementary switches</text>
  <text x="250" y="66" text-anchor="middle" class="hi">VDD = 1.8 V</text>
  <path class="wire" d="M210 70 H290"/>
  {mos(200, 110, True)}
  {mos(200, 230, False)}
  <path class="wire" d="M170 110 V230 M110 170 H170"/>
  <circle cx="170" cy="170" r="3.5" fill="#c9cde6"/>
  <path class="wire" d="M246 150 V190 M246 170 H330"/>
  <circle cx="246" cy="170" r="3.5" fill="#c9cde6"/>
  <path class="wire" d="M210 270 H290 M224 278 H276 M238 286 H262"/>
  <text x="250" y="304" text-anchor="middle">GND</text>
  <text x="290" y="114" class="hi">PMOS</text>
  <text x="290" y="234" class="hi">NMOS</text>
  <text x="100" y="160" text-anchor="end" class="hi">in</text>
  <text x="336" y="160" class="hi">out</text>

  <g class="s0 a">
    <path class="on" d="M216 90 V130 M224 90 V130"/>
    <path class="cur" d="M254 70 V162 H330"/>
    <text x="100" y="196" text-anchor="end" class="big mono" style="fill:#5cc0ff">0</text>
    <text x="336" y="198" class="big mono" style="fill:#9d90ff">1</text>
    <text x="44" y="326">in = 0: PMOS on, NMOS off, out pulled up to VDD</text>
  </g>
  <g class="s1 a">
    <path class="on" d="M216 210 V250 M224 210 V250"/>
    <path class="cur" d="M330 178 H254 V270"/>
    <text x="100" y="196" text-anchor="end" class="big mono" style="fill:#5cc0ff">1</text>
    <text x="336" y="198" class="big mono" style="fill:#9d90ff">0</text>
    <text x="44" y="326">in = 1: NMOS on, PMOS off, out pulled down to GND</text>
  </g>

  <rect x="424" y="20" width="352" height="320" rx="10" fill="#161a2c"/>
  <text x="{X0}" y="46" class="title">Voltage transfer curve (simulated)</text>
  <path class="grid" d="M{X0} {py(0.9):.0f} H{X1} M{px(0.9):.0f} {Y0} V{Y1}" stroke-dasharray="3 5"/>
  <path d="M{X0} {Y1} L{X1} {Y0}" stroke="#3a4066" stroke-dasharray="2 4"/>
  <path id="vtc" class="vtc" d="{vtc}"/>
  <circle cx="{px(0.82):.1f}" cy="{py(0.82):.1f}" r="4" fill="#f5b83d"/>
  <text x="{px(0.82) + 8:.1f}" y="{py(0.82) - 8:.1f}" style="fill:#f5b83d">V<tspan dy="3" style="font-size:9px">M</tspan><tspan dy="-3"> ≈ 0.82 V</tspan></text>
  <circle r="6" fill="#e6e8f2" stroke="#9d90ff" stroke-width="2">
    <animateMotion dur="8s" repeatCount="indefinite" calcMode="linear" keyPoints="0;0;1;1;0" keyTimes="0;0.4;0.5;0.9;1">
      <mpath href="#vtc"/>
    </animateMotion>
  </circle>
  <path class="axis" d="M{X0} {Y0} V{Y1} H{X1}"/>
  <g text-anchor="middle"><text x="{X0}" y="{Y1 + 18}">0</text><text x="{px(0.9):.0f}" y="{Y1 + 18}">0.9</text><text x="{X1}" y="{Y1 + 18}">1.8</text>
    <text x="{(X0 + X1) / 2:.0f}" y="{Y1 + 34}" class="hi">v(in) (V)</text></g>
  <g text-anchor="end"><text x="{X0 - 6}" y="{Y1 + 4}">0</text><text x="{X0 - 6}" y="{py(0.9) + 4:.0f}">0.9</text><text x="{X0 - 6}" y="{Y0 + 4}">1.8</text></g>
  <text x="{X0 + 150}" y="{Y0 + 30}" class="hi">v(out)</text>"""
    svg("learn-cmos-inverter.svg", 800, 360,
        "A CMOS inverter drawn as two complementary switches: a low input turns the PMOS on and pulls the output high, a high input turns the NMOS on and pulls it low; a dot moves along the simulated transfer curve",
        style, body)


# ---------------------------------------------------------------- NAND2
def nand2():
    rows = [("0", "0", "1"), ("0", "1", "1"), ("1", "0", "1"), ("1", "1", "0")]
    # pmos A at x=150, pmos B at x=290 (parallel); nmos A at y=220, nmos B at y=290 (series)
    def pm(x, y, name):
        return (f'<path class="wire" d="M{x - 36} {y} H{x - 8}"/><circle cx="{x - 4}" cy="{y}" r="4" fill="none" stroke="#c9cde6" stroke-width="2"/>'
                f'<path class="wire" d="M{x} {y - 16} V{y + 16} M{x + 8} {y - 18} V{y + 18} M{x + 8} {y - 12} H{x + 26} V{y - 36} M{x + 8} {y + 12} H{x + 26} V{y + 36}"/>'
                f'<text x="{x - 40}" y="{y + 4}" text-anchor="end" class="hi">{name}</text>')
    def nm(x, y, name):
        return (f'<path class="wire" d="M{x - 36} {y} H{x}"/>'
                f'<path class="wire" d="M{x} {y - 16} V{y + 16} M{x + 8} {y - 18} V{y + 18} M{x + 8} {y - 12} H{x + 26} V{y - 24} M{x + 8} {y + 12} H{x + 26} V{y + 24}"/>'
                f'<text x="{x - 40}" y="{y + 4}" text-anchor="end" class="hi">{name}</text>')
    hl = {"pA": (150, 110), "pB": (290, 110), "nA": (220, 226), "nB": (220, 286)}
    phases = []
    for n, (a, b, y) in enumerate(rows):
        on = []
        if a == "0": on.append("pA")
        if b == "0": on.append("pB")
        if a == "1": on.append("nA")
        if b == "1": on.append("nB")
        rects = "".join(f'<rect x="{hl[t][0] - 6}" y="{hl[t][1] - 22}" width="40" height="44" rx="6" fill="rgba(70,208,138,0.18)" stroke="#46d08a" stroke-width="2"/>' for t in on)
        if y == "1":
            paths = []
            if a == "0": paths.append("M176 74 V170")
            if b == "0": paths.append("M316 74 V170")
            cur = f'<path class="cur" d="{" ".join(paths)} M176 170 H380"/>'
            txt = "a PMOS conducts: Y pulled up to 1"
        else:
            cur = '<path class="cur" d="M380 170 H246 V310"/>'
            txt = "both NMOS conduct: Y pulled down to 0"
        ry = 120 + n * 40
        phases.append(f"""  <g class="ph a" style="animation-delay:{n * 2.5:.1f}s">
    {rects}
    {cur}
    <rect x="520" y="{ry - 26}" width="220" height="36" rx="6" fill="rgba(157,144,255,0.18)" stroke="#9d90ff"/>
    <text x="110" y="136" text-anchor="end" class="mono big" style="fill:#5cc0ff">{a}</text>
    <text x="250" y="136" text-anchor="end" class="mono big" style="fill:#5cc0ff">{b}</text>
    <text x="180" y="250" text-anchor="end" class="mono big" style="fill:#5cc0ff">{a}</text>
    <text x="180" y="310" text-anchor="end" class="mono big" style="fill:#5cc0ff">{b}</text>
    <text x="396" y="200" class="mono big" style="fill:#9d90ff">{y}</text>
    <text x="44" y="356">A={a}, B={b}: {txt}</text>
  </g>""")
    table = "".join(
        f'<text x="560" y="{120 + n * 40}" class="mono hi" text-anchor="middle">{a}</text>'
        f'<text x="630" y="{120 + n * 40}" class="mono hi" text-anchor="middle">{b}</text>'
        f'<text x="700" y="{120 + n * 40}" class="mono hi" text-anchor="middle">{y}</text>'
        for n, (a, b, y) in enumerate(rows)
    )
    style = """
    .ph { opacity: 0; animation-name: ph; animation-duration: 10s; }
    @keyframes ph { 0% { opacity: 0; } 2%, 23% { opacity: 1; } 25%, 100% { opacity: 0; } }
    .cur { stroke: #46d08a; stroke-width: 2; stroke-dasharray: 5 6; fill: none; animation: flow 0.8s linear infinite; }
    @keyframes flow { to { stroke-dashoffset: -11; } }
    .big { font-size: 18px; font-weight: 700; }
"""
    body = f"""  <rect x="24" y="20" width="470" height="350" rx="10" fill="#161a2c"/>
  <text x="44" y="46" class="title">NAND2: PMOS in parallel, NMOS in series</text>
  <text x="246" y="64" text-anchor="middle" class="hi">VDD</text>
  <path class="wire" d="M150 74 H340"/>
  {pm(150, 110, "A")}
  {pm(290, 110, "B")}
  <path class="wire" d="M176 146 V170 H316 V146 M246 170 V202 M246 170 H380"/>
  <circle cx="246" cy="170" r="3.5" fill="#c9cde6"/><circle cx="176" cy="170" r="3.5" fill="#c9cde6"/>
  <text x="386" y="174" class="hi">Y</text>
  {nm(220, 226, "A")}
  {nm(220, 286, "B")}
  <path class="wire" d="M246 250 V262"/>
  <path class="wire" d="M246 310 V322 M226 322 H266 M234 328 H258 M242 334 H250"/>

  <rect x="504" y="20" width="272" height="350" rx="10" fill="#161a2c"/>
  <text x="524" y="46" class="title">Truth table</text>
  <text x="560" y="80" text-anchor="middle" class="hi">A</text><text x="630" y="80" text-anchor="middle" class="hi">B</text><text x="700" y="80" text-anchor="middle" class="hi">Y</text>
  <path d="M524 90 H756" stroke="#3a4066"/>
  {table}
  <text x="524" y="300">Y = NOT (A AND B)</text>
  <text x="524" y="320">Y is 0 only when both</text>
  <text x="524" y="338">inputs are 1</text>
{"".join(phases)}"""
    svg("learn-nand2.svg", 800, 380,
        "A two-input NAND gate stepping through its truth table: the conducting transistors and the current path are highlighted for each input combination",
        style, body)


# ---------------------------------------------------------------- layer stack
def layer_stack():
    steps = [
        ("substrate", "p-type silicon wafer", '<rect x="40" y="250" width="500" height="90" fill="#2a2f4a" stroke="#5a6190"/>'),
        ("nwell", "n-well: a home for the PMOS",
         '<path d="M300 250 V300 Q300 316 316 316 H524 Q540 316 540 300 V250 Z" fill="rgba(120,200,120,0.25)" stroke="#78c878" stroke-dasharray="4 3"/>'),
        ("active", "active: where transistors can be",
         '<rect x="60" y="244" width="200" height="12" fill="rgba(70,190,90,0.55)" stroke="#46be5a"/><rect x="320" y="244" width="200" height="12" fill="rgba(70,190,90,0.55)" stroke="#46be5a"/>'),
        ("poly", "poly: the transistor gates",
         '<rect x="140" y="226" width="40" height="18" fill="rgba(230,60,60,0.8)" stroke="#e63c3c"/><rect x="400" y="226" width="40" height="18" fill="rgba(230,60,60,0.8)" stroke="#e63c3c"/>'
         '<rect x="138" y="243" width="44" height="3" fill="#9aa3d0"/><rect x="398" y="243" width="44" height="3" fill="#9aa3d0"/>'),
        ("nplus / pplus", "implants: n+ and p+ source and drain",
         '<path d="M68 256 V268 Q68 276 76 276 H130 Q138 276 138 268 V256 Z M182 256 V268 Q182 276 190 276 H244 Q252 276 252 268 V256 Z" fill="rgba(230,120,60,0.7)" stroke="#e6783c"/>'
         '<path d="M328 256 V268 Q328 276 336 276 H390 Q398 276 398 268 V256 Z M442 256 V268 Q442 276 450 276 H504 Q512 276 512 268 V256 Z" fill="rgba(200,160,60,0.7)" stroke="#c8a03c"/>'
         '<text x="103" y="272" text-anchor="middle" class="hi" style="font-size:10px">n+</text><text x="217" y="272" text-anchor="middle" class="hi" style="font-size:10px">n+</text>'
         '<text x="363" y="272" text-anchor="middle" class="hi" style="font-size:10px">p+</text><text x="477" y="272" text-anchor="middle" class="hi" style="font-size:10px">p+</text>'),
        ("contact", "contacts: holes down to silicon",
         '<rect x="95" y="196" width="16" height="48" fill="#ebebeb"/><rect x="209" y="196" width="16" height="48" fill="#ebebeb"/>'
         '<rect x="355" y="196" width="16" height="48" fill="#ebebeb"/><rect x="469" y="196" width="16" height="48" fill="#ebebeb"/>'),
        ("metal1", "metal1: the first wiring layer",
         '<rect x="80" y="180" width="46" height="16" fill="rgba(80,140,250,0.75)" stroke="#508cfa"/><rect x="340" y="180" width="46" height="16" fill="rgba(80,140,250,0.75)" stroke="#508cfa"/>'
         '<rect x="196" y="180" width="304" height="16" fill="rgba(80,140,250,0.75)" stroke="#508cfa"/>'),
        ("via1", "via1: metal1 to metal2",
         '<rect x="420" y="150" width="16" height="30" fill="#fad250"/>'),
        ("metal2", "metal2: wiring above metal1",
         '<rect x="300" y="134" width="220" height="16" fill="rgba(210,100,230,0.7)" stroke="#d264e6"/>'),
    ]
    n = len(steps)
    period = 14.0
    keyframes, groups, legend = [], [], []
    for i, (name, desc, shapes) in enumerate(steps):
        t = 4 + i * 8.5  # percent
        keyframes.append(f"@keyframes L{i} {{ 0%, {t:.1f}% {{ opacity: 0; }} {t + 3:.1f}%, 92% {{ opacity: 1; }} 97%, 100% {{ opacity: 0; }} }}")
        keyframes.append(f"@keyframes H{i} {{ 0%, {t:.1f}% {{ opacity: 0; }} {t + 2:.1f}%, {t + 8.5:.1f}% {{ opacity: 1; }} {t + 10:.1f}%, 100% {{ opacity: 0; }} }}")
        groups.append(f'<g class="a" style="animation-name:L{i};animation-duration:{period}s">{shapes}</g>')
        y = 70 + i * 30
        legend.append(
            f'<rect class="a" x="572" y="{y - 17}" width="196" height="26" rx="5" fill="rgba(157,144,255,0.18)" stroke="#9d90ff" '
            f'style="animation-name:H{i};animation-duration:{period}s"/>'
            f'<text x="582" y="{y}" class="mono hi">{i + 1}. {name}</text>'
        )
        if i == n - 1:
            pass
    captions = "".join(
        f'<text class="a" x="290" y="370" text-anchor="middle" style="animation-name:H{i};animation-duration:{period}s;fill:#e6e8f2">{desc}</text>'
        for i, (_, desc, _) in enumerate(steps)
    )
    style = "\n    " + "\n    ".join(keyframes) + "\n"
    body = f"""  <rect x="24" y="20" width="532" height="364" rx="10" fill="#161a2c"/>
  <text x="44" y="46" class="title">Building an inverter, one mask layer at a time</text>
  <text x="160" y="104" text-anchor="middle">NMOS</text><text x="420" y="104" text-anchor="middle">PMOS</text>
  {''.join(groups)}
  {captions}
  <rect x="562" y="20" width="214" height="364" rx="10" fill="#161a2c"/>
  <text x="578" y="46" class="title">Layers (masks)</text>
  {''.join(legend)}"""
    svg("learn-layer-stack.svg", 800, 400,
        "Cross-section of a CMOS inverter built up layer by layer: substrate, n-well, active, poly gates, n+ and p+ implants, contacts, metal1, via1 and metal2",
        style, body)


# ---------------------------------------------------------------- common-source amplifier
def cs_amp():
    # two scrolling sine waves
    def sine(x0, yc, amp, wl, n, phase=0):
        pts = []
        for i in range(n * 40 + 1):
            x = x0 + wl * i / 40
            pts.append((x, yc - amp * math.sin(2 * math.pi * i / 40 + phase)))
        return path(pts)
    W = 120
    style = """
    .tin { fill: none; stroke: #5cc0ff; stroke-width: 2.2; }
    .tout { fill: none; stroke: #9d90ff; stroke-width: 2.4; }
    .scroll { animation: scroll 2.4s linear infinite; }
    @keyframes scroll { from { transform: translateX(0); } to { transform: translateX(-120px); } }
    .beat { animation: beat 2.4s linear infinite; }
"""
    body = f"""  <rect x="24" y="20" width="330" height="320" rx="10" fill="#161a2c"/>
  <text x="44" y="46" class="title">Common-source amplifier</text>
  <text x="230" y="72" text-anchor="middle" class="hi">VDD = 1.8 V</text>
  <path class="wire" d="M190 80 H270 M230 80 V96"/>
  <path class="wire" d="M230 96 l-9 6 l18 10 l-18 10 l18 10 l-18 10 l18 10 l-9 6 V176"/>
  <text x="250" y="140" class="hi">R<tspan dy="3" style="font-size:9px">D</tspan><tspan dy="-3"> = 50 kΩ</tspan></text>
  <circle cx="230" cy="176" r="3.5" fill="#9d90ff"/>
  <path class="wire" d="M230 176 H310"/><text x="316" y="180" class="hi">out</text>
  <path class="wire" d="M230 176 V200 H212 M204 196 V244 M212 200 V240 M212 240 H230 V270"/>
  <path class="wire" d="M150 220 H204"/>
  <text x="246" y="226" class="hi">M1</text>
  <text x="246" y="244" style="font-size:11px">W/L = 2 µm / 1 µm</text>
  <path class="wire" d="M210 270 H250 M218 277 H242 M226 284 H234"/>
  <circle cx="110" cy="220" r="22" fill="none" stroke="#5cc0ff" stroke-width="2"/>
  <path d="M96 220 q7 -12 14 0 t14 0" stroke="#5cc0ff" stroke-width="1.8" fill="none"/>
  <path class="wire" d="M132 220 H150 M110 242 V270 H210"/>
  <text x="110" y="186" text-anchor="middle" class="hi">in</text>
  <text x="44" y="306">0.7 V bias + 10 mV signal at the gate</text>
  <text x="44" y="324">→ id = gm · vin flows through R<tspan dy="3" style="font-size:9px">D</tspan></text>

  <rect x="364" y="20" width="412" height="320" rx="10" fill="#161a2c"/>
  <text x="384" y="46" class="title">Small signal in, bigger signal out</text>
  <defs><clipPath id="w"><rect x="420" y="60" width="180" height="270"/></clipPath></defs>
  <text x="384" y="84" class="hi">v(in)</text><text x="384" y="102" style="font-size:11px">±10 mV</text>
  <text x="384" y="214" class="hi">v(out)</text><text x="384" y="232" style="font-size:11px">±70 mV</text>
  <path class="grid" d="M420 120 H600 M420 220 H600" stroke-dasharray="3 5"/>
  <g clip-path="url(#w)">
    <g class="scroll"><path class="tin" d="{sine(420, 120, 10, W, 5)}"/></g>
    <g class="scroll"><path class="tout" d="{sine(420, 220, 70, W, 5, math.pi)}"/></g>
  </g>
  <path d="M450 90 V300" stroke="#f5b83d" stroke-width="1" stroke-dasharray="3 3"/>
  <text x="456" y="318" style="fill:#f5b83d">peak in = trough out: the stage inverts</text>
  <rect x="610" y="142" width="152" height="56" rx="8" fill="#0f1220" stroke="#3a4066"/>
  <text x="686" y="164" text-anchor="middle" class="mono hi">A = −gm·R<tspan dy="3" style="font-size:9px">D</tspan></text>
  <text x="686" y="186" text-anchor="middle" class="mono" style="fill:#9d90ff">≈ −7 (17 dB)</text>"""
    svg("learn-cs-amp.svg", 800, 360,
        "A common-source amplifier: a 10 millivolt sine at the gate becomes a 70 millivolt inverted sine at the drain",
        style, body)


chip_zoom()
rc_charge()
mosfet_channel()
mosfet_iv()
cmos_inverter()
nand2()
layer_stack()
cs_amp()
print("ok")
