<!-- seo title="Python API" description="Python에서 IC Studio 스크립팅: 프로젝트 열기, 회로도 작성, 전체 파형 시뮬레이션, DRC·LVS·검증 실행, 모든 MCP 도구 호출." -->

# Python API

`icstudio` Python 패키지를 쓰면 스크립트와 노트북에서 IC Studio 프로젝트를 열 수 있습니다. 넷리스트로 회로도를
만들고, 전체 파형으로 시뮬레이션하고, DRC, LVS, 검증 계획을 실행하고, [MCP 도구](mcp.md)를 무엇이든 호출할 수
있습니다. 앱과 `ics` 명령줄이 쓰는 Rust 코드를 같은 프로세스 안에서 실행하므로, 앱을 띄워 둘 필요가 없습니다.

## 설치

패키지는 저장소에서 [maturin](https://www.maturin.rs)으로 빌드합니다. Rust([설치](install.md) 참고)와
Python 3.9 이상이 필요합니다.

```sh
pip install ./python            # 저장소 체크아웃에서
pip install numpy               # 선택: 파형을 NumPy 배열로
```

## 첫 스크립트

```python
import icstudio

p = icstudio.Project.open("myproj")      # 기존 프로젝트
# p = icstudio.Project.create("newproj", template="empty")   # 새 프로젝트(바로 저장됨)
# p = icstudio.Project.demo()            # 메모리 속 데모 셀

print(p.cells())                         # ['work/ota5t', 'work/rc_lowpass', ...]

p.write_schematic("work/divider", """
V1 a 0 1.8
R1 a b 10k
R2 b 0 10k
.op
""")
r = p.simulate("divider")
print(r["op"]["vectors"]["v(b)"])        # [0.9]
p.save()
```

변경 사항은 `save()`(다른 곳에 저장하려면 `save("other/dir")`)를 부를 때까지 메모리에만 있습니다.

## 시뮬레이션 결과

`simulate(cell)`은 셀에 저장된 해석을 실행하고 전체 파형을 돌려줍니다. 결과는 해석 이름(`op`, `dc`, `ac`,
`tran`, `noise`)을 키로 하는 dict입니다.

```python
r = p.simulate("rc_lowpass", backend="builtin")    # backend: builtin, ngspice, auto
ac = r["ac"]
f = ac["vectors"][ac["scale"]]           # 스윕 변수: 주파수
h = ac["vectors"]["v(out)"]              # AC는 복소수

import numpy as np
fc = f[np.argmin(abs(abs(h) - 1 / np.sqrt(2)))]    # -3 dB 주파수
```

각 해석에는 `title`, `scale`(스윕 벡터 이름), `vectors`(이름별 값)가 있고, 동작점 해석에는 `devices`(소자별
동작점 값과 영역)도 있습니다. NumPy가 설치되어 있으면 벡터는 NumPy 배열이고, 없으면 리스트입니다.
`simulate(cell, parasitics=True)`는 레이아웃에서 추출한 기생 성분을 포함합니다. `icstudio.run_spice(deck)`는
SPICE나 Spectre 덱을 그 덱의 해석으로 시뮬레이션하고 같은 형태로 돌려줍니다.

## 메서드

| 메서드 | 기능 |
| --- | --- |
| `Project.open(path)`, `Project.create(path, template)`, `Project.demo()` | 열기, 만들기(`demo` 또는 `empty`), 데모 불러오기 |
| `name`, `root`, `cells()`, `info()` | 프로젝트 이름, 디렉터리, 셀(`lib/cell`), 라이브러리와 테크놀로지 |
| `netlist(cell, format)` | 넷리스트: `spice`(해석 포함), `spectre`, `cdl` |
| `write_schematic(cell, netlist, overwrite)` | 평탄 SPICE 넷리스트로 회로도 만들기 |
| `simulate(cell, parasitics, backend)` | 저장된 해석의 전체 파형 |
| `drc(cell)`, `lvs(cell)` | 테크놀로지 덱으로 DRC, 레이아웃 대 회로도 |
| `verify(tests, save)` | [검증 계획](simulation.md#검증전체-테스트): 스펙별 worst 값, 마진, 상태 |
| `skill(code)` | 프로젝트에서 Cadence SKILL 실행 |
| `call(tool, **args)` | 이름으로 MCP 도구 호출 |
| `save(path)` | 저장(다른 이름으로 저장) |

## 모든 MCP 도구

`call`은 [MCP 서버](mcp.md)의 도구를 키워드 인자로 실행하고, JSON 결과를 Python dict와 list로 돌려줍니다.
`icstudio.tools()`는 도구 목록을 설명, 인자 스키마와 함께 보여 줍니다.

```python
p = icstudio.Project.demo()
p.call("place_pcell", cell="cmos_inverter", kind="nmos", params={"w": 2, "l": 0.18}, x=10, y=0)
p.call("route", cell="cmos_inverter")
p.call("export_gds", cell="cmos_inverter", path="inv.gds")
[t["name"] for t in icstudio.tools()]
```

도구가 실패하면 그 메시지와 함께 `icstudio.IcsError`가 발생합니다.

설치된 [확장](extensions.md)의 도구도 전체 이름으로 같은 방법으로 호출하며
(`p.call("design-report__report", cell="nand2")`), `icstudio.tools()` 목록에도 나옵니다.

## 예: CI에서 regression 검사

```python
import sys
import icstudio

run = icstudio.Project.open("myproj").verify(save=True)
for t in run["tests"]:
    for s in t["specs"]:
        print(f'{t["name"]:12} {s["name"]:10} {s["status"]:5} worst {s["worst"]} at {s["at"]}')
regressions = [c for c in run["changes"] if c["regression"]]
sys.exit(1 if run["status"] in ("FAIL", "error") or regressions else 0)
```

셸에서는 `ics verify`가 같은 일을 하고 JUnit XML도 씁니다([명령줄](cli.md#검증) 참고).
