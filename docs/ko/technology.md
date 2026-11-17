<!-- seo title="공정 기술과 PDK 설정 (SKY130, IHP SG13G2)" description="IC Studio의 공정 기술 설정: 레이어, GDS 번호, DRC 룰 덱, 비아, SKY130과 IHP SG13G2 같은 오픈 PDK 가져오기." -->

# 테크놀로지

프로젝트의 테크놀로지(PDK)는 프로젝트 폴더의 `tech.json`에 들어 있습니다. 레이어와 GDS 번호,
표시 스타일, 간단한 폭 / 간격 룰, 비아, 기생 성분 추출용 전기 계수, DRC 룰 덱을 정의합니다.
`tech.json`이 없는 프로젝트는 내장 `generic180` 테크놀로지를 사용합니다.

## generic180

새 프로젝트와 데모에서 쓰는 일반적인 180 nm급 테크놀로지입니다. 1 dbu = 1 nm, 그리드 10 nm입니다.

| 레이어 | GDS | 레이어 | GDS |
|---|---|---|---|
| nwell | 1 | via1 | 8 |
| active | 2 | metal2 | 9 |
| nplus | 3 | via2 | 10 |
| pplus | 4 | metal3 | 11 |
| poly | 5 | boundary | 62 |
| contact | 6 | text | 63 |
| metal1 | 7 | poly2 | 12 |

배선 레이어는 poly와 metal1~metal3입니다. 비아: `contact`(active에서 metal1), `polycon`(poly에서
metal1), `poly2con`(poly2에서 metal1), `via1`, `via2`. `poly2`는 첫 번째 poly 위의 두 번째 poly이며,
`poly`와 겹치는 곳을 추출기가 커패시터 `cpip`(층간 ONO, 2.65 fF/µm², 등가 산화막 13 nm)로
인식합니다. 플래시 예제 `nand_string`의 컨트롤 게이트와 플로팅 게이트 사이 커패시턴스입니다.

## tech.json

최상위:

| 필드 | 의미 |
|---|---|
| `name` | 테크놀로지 이름 |
| `dbu_per_um` | µm당 데이터베이스 단위(1000이면 1 dbu = 1 nm) |
| `grid` | 스냅 그리드(dbu) |
| `layers` | 레이어 배열 |
| `vias` | 비아 정의 배열 |
| `drc_deck` | DRC 덱 텍스트(선택. 없으면 레이어 룰에서 유도) |
| `caps` | 레이어별 기생 계수(선택) |
| `roles` | 어떤 레이어와 비아가 확산, 폴리, 웰, 임플란트, 콘택인지(선택. 기본값은 generic180 이름) |
| `rules` | [PCell](pcells.md#테크놀로지가-제공해야-하는-항목)이 쓰는 소자 룰, dbu 단위(선택) |
| `devices` | PDK MOS 소자(선택. 없으면 일반 `nmos` / `pmos`) |
| `models` | PDK SPICE 모델 라이브러리(선택. 없으면 내장 level-1 모델) |
| `signoff` | 외부 DRC / LVS 런셋(선택. [검증](verification.md#외부-도구로-사인오프) 참고) |
| `pcell_layers`, `pin_labels`, `drc_extra` | 추가 PCell 레이어, 넷 라벨 레이어, 덱 줄. [PDK 파일](#pdk-파일)의 `[[pcell_layer]]`, `[[pin_label]]`, `drc_extra`와 같음(선택) |
| `stream_map` | 가져오기와 내보내기용 GDSII / OASIS 레이어 맵: `layer`, `stream_layer`, `datatype` 행(선택. [Stream Layer Map](tools.md#stream-layer-map) 참고) |

레이어:

```json
{
  "name": "metal1", "gds_layer": 7, "gds_datatype": 0,
  "color": [80, 140, 255, 160], "fill": "hatch",
  "min_width": 230, "min_spacing": 230, "routing": true
}
```

- 표시는 Cadence Virtuoso 디스플레이 패킷 방식을 따릅니다. 도형은 레이어 색의 스티플 비트맵으로
  그려지며, 비트 하나가 화면 포인트 하나이고 창 기준으로 고정되어 이웃하거나 겹친 도형의 패턴이
  맞물립니다. 외곽선은 외곽선 색과 선 스타일로 그립니다. Virtuoso처럼 스티플 픽셀은 불투명합니다.
  - `fill`: `solid`, `outline`, `cross`(Virtuoso 채움 스타일 X: 외곽선과 두 대각선, 컷 레이어용) 또는
    스티플 `hatch`(`/`), `backhatch`(`\`), `crosshatch`, `hline`, `vline`, `grid`, `dots`,
    `sparsedots`, `checker`, `brick`, `zigzag`.
  - `stipple`(선택): 스티플 채움에 쓰는 사용자 비트맵. `0` / `1` 행, 예: `["1000", "0100", "0010", "0001"]`.
  - `outline`(선택): 외곽선 색 `[r, g, b]`. 기본값은 채움 색.
  - `line`(선택): `solid`, `dashed`, `shortdash`, `dotted`, `dashdot`, `doubledash`, `thick`, `none`.
- `min_width`, `min_spacing`: dbu 단위. 0이면 검사하지 않습니다. `min_area`(선택): dbu² 단위.
  PCell 콘택 판과 라우터 비아 패드가 이 값에 도달하도록 커집니다.
- `routing`: 배선 레이어는 넷 라벨을 담고 추출과 라우터에서 사용됩니다.

비아: `name`, `bottom`, `cut`, `top`(레이어 이름), `cut_size`, `cut_spacing`, `enc_bottom`,
`enc_top`(dbu), `resistance`(컷당 Ω), `em`(선택. 컷당 A 단위의 일렉트로마이그레이션 한계,
[IR 강하와 일렉트로마이그레이션](verification.md#ir-강하와-일렉트로마이그레이션) 참고).

기생 계수(`caps`)는 레이어마다 한 항목입니다: `layer`, `area`(aF/µm²), `fringe`(변 µm당 aF),
`coupling`(aF. 평행한 이웃에 대해 `coupling * length / spacing`을 더함), `resistance`(스퀘어당
Ω), `em`(선택. 배선 폭 µm당 A 단위의 일렉트로마이그레이션 한계).

## DRC 룰 덱

덱은 한 줄에 구문 하나씩 쓰는 일반 텍스트입니다. `#`은 주석을 시작합니다. 길이는 µm, 면적은 µm²
단위입니다. 키워드와 레이어 이름은 대소문자를 구분하지 않습니다. 모든 룰에 선택적으로 `NAME:`
접두사를 붙일 수 있으며, 없으면 `metal1.W`나 `contact.EN.metal1` 같은 이름이 생성됩니다. `#@ text`
줄은 다음 룰의 설명이 되어, 생성된 설명 대신 위반과 함께 보고됩니다.

### 룰

| 구문 | 검사 내용 |
|---|---|
| `width LAYER MIN` | 최소 폭 |
| `space LAYER MIN` | 한 레이어 도형 사이의 최소 간격 |
| `separation A B MIN` | A와 B 사이의 최소 간격(A와 겹치지 않는 B 조각) |
| `enclosure INNER OUTER MIN` | OUTER가 INNER를 MIN만큼 감싸야 함(OUTER와 겹치는 INNER 조각) |
| `extension A B MIN` | A가 B 밖으로 MIN만큼 연장되어야 함(예: active 위 poly 엔드캡) |
| `area LAYER MIN` | 최소 면적 |
| `antenna METAL RATIO` | 연결된 게이트 면적 대비 메탈 면적의 최대 비율(`poly`와 `active` 레이어 필요) |
| `density LAYER MIN MAX WINDOW [STEP]` | WINDOW x WINDOW µm 슬라이딩 윈도 안에서 밀도가 MIN~MAX 퍼센트(단위 없는 숫자, 0~100) 사이여야 함(STEP 기본값은 WINDOW/2) |
| `flag LAYER` | LAYER의 모든 조각이 위반(유도 레이어와 함께 사용) |

### 유도 레이어

```text
NAME = A and B
NAME = A or B
NAME = A not B        # A에서 B를 뺌
NAME = A xor B
NAME = A size DIST    # 키움(DIST가 음수면 줄임)
NAME = A wide MIN     # A에서 폭이 MIN 이상인 부분
NAME = A interact B   # B와 겹치거나 닿는 A 조각
NAME = A inside B     # B 안에 완전히 들어 있는 A 조각
NAME = A outside B    # B와 겹치지 않는 A 조각
NAME = A enclose B    # B 조각 하나를 통째로 포함하는 A 조각
NAME = A touch B      # B와 겹치지 않고 닿기만 하는 A 조각
NAME = A not interact B   # (not inside, not outside, not enclose, not touch도 가능)
NAME = A area MIN MAX # MIN <= 면적 < MAX인 조각(µm². MAX는 inf 가능)
NAME = holes A        # A의 각 조각이 둘러싼 빈 영역
NAME = extents A      # A의 각 조각의 바운딩 박스
```

조각(piece)은 레이어에서 변으로 연결된 부분입니다. 유도 레이어는 테크놀로지 레이어와 앞서 정의한
유도 레이어를 사용할 수 있고, 어떤 룰에서든 쓸 수 있습니다. `flag`를 쓰면 어떤 유도 레이어든
검사가 됩니다. 예를 들어 떠 있는 폴리는 다음과 같습니다.

```text
floating = poly not interact active
PO.FLOAT: flag floating
```

### SVRF 덱

SVRF(Calibre 룰 언어)로 작성한 덱은 자동으로 인식되어 위 룰로 변환되므로, 파운드리 스타일 덱을
내장 엔진에서 실행할 수 있습니다. 덱을 쓰는 모든 곳에서 동작합니다: **Rule deck** 편집기,
`tech.json`이나 PDK 파일의 `drc_deck`, `ics drc --deck rules.svrf`, `tool = "builtin"`인 런셋.
파일에서 읽을 때 `INCLUDE`는 그 파일 기준 상대 경로로 해석됩니다.

변환되는 것:

- 주석, `#DEFINE` / `#UNDEFINE` / `#IFDEF` / `#IFNDEF` / `#ELSE` / `#ENDIF`, 숫자 `VARIABLE`.
- `LAYER MAP g DATATYPE d n`과 `LAYER name n ...`. 레이어 번호는 그 GDS layer와 datatype을 가진
  테크놀로지 레이어에 매핑됩니다(`LAYER MAP`이 없으면 모든 datatype).
- 레이어 연산 `AND`, `OR`, `NOT`, `XOR`, `[NOT] INTERACT / INSIDE / OUTSIDE / ENCLOSE /
  TOUCH`, `SIZE a BY d [OVERUNDER | UNDEROVER]`, `HOLES`, `EXTENTS`, `COPY`, `AREA`, 괄호.
- 룰 검사 `NAME { @ comment ... }`와 `INT a < d`(width), `EXT a < d`(space),
  `EXT a b < d`(separation), `ENC a b < d`(enclosure), `AREA a < x`,
  `DENSITY a < x | > x WINDOW w STEP s`, 또는 임의의 레이어 식(식의 모든 도형이 보고됨). `@`
  주석은 위반의 설명이 됩니다.

사양 및 LVS 구문(`LAYOUT`, `DRC`, `PRECISION`, `CONNECT`, `DEVICE` 등)은 건너뜁니다. 측정 방식을
바꾸는 검사 수식어(`OPPOSITE`, `PROJECTING`, `PARALLEL` 등)는 안내 메시지와 함께 무시하고, 출력
형태만 바꾸는 수식어(`REGION`, `SINGULAR`, `ABUT`)는 조용히 무시합니다. 엣지 연산, `WITH WIDTH`,
두 레이어 `INT` 같은 나머지는 해당 룰 검사를 제외합니다. 이런 경우는 모두 줄 번호와 함께
나열됩니다: 룰 덱 아래의 **SVRF notes**, `ics drc`의 `note:` 줄, MCP의 `deck_notes`. 내장 측정은
맨해튼이며 모서리에서는 유클리드 거리를 쓰므로, 사인오프는 같은 덱을 Calibre에서 실행하세요
([검증](verification.md#외부-도구로-사인오프) 참고).

### 예제

```text
# 폴리와 게이트
width poly 0.18
space poly 0.24
gate = poly and active
GATE.EX: extension poly active 0.22
width gate 0.18

# metal1, 넓은 메탈 옆에서는 더 큰 간격
width metal1 0.23
space metal1 0.23
m1w = metal1 wide 10
M1.WIDE.S: separation m1w metal1 0.6
area metal1 0.2

# 콘택과 비아
enclosure contact active 0.1
enclosure contact metal1 0.06
enclosure via1 metal1 0.06

antenna metal1 400
density metal1 20 80 100
```

테크놀로지에 `drc_deck`이 없으면 IC Studio는 레이어의 `min_width`, `min_spacing`, `min_area`,
비아 컷 크기, 간격, 인클로저, active 위 poly 연장, 안테나 룰, PDK의 `drc_extra` 줄로부터 덱을
유도합니다. Layout 워크스페이스에서 **Rule deck**을 열면 덱을 보고 편집하고 **Save to technology**로
저장할 수 있습니다. [DRC Rule Designer](tools.md#drc-rule-designer)는 같은 덱을 폼으로 편집합니다.
명령줄에서는 별도 파일을 쓸 수 있습니다: `ics drc <project> <cell> --deck rules.drc`.

## PDK 파일

PDK 파일은 공정 전체를 기술하는 TOML 파일 하나입니다. 룰을 포함한 레이어, 비아, 레이어 역할, MOS
소자, SPICE 모델 라이브러리가 들어갑니다. 저장소에는 `pdks/sky130A.toml`(SkyWater SKY130)과
`pdks/ihp-sg13g2.toml`(IHP SG13G2, 1.2 V 및 3.3 V MOS 소자, Metal1~TopMetal2)이 포함되어 있습니다.
둘 다 `$PDK_ROOT`를 통해 설치된 PDK에서 모델과 사인오프 덱을 읽습니다.

- GUI: **File > Import > PDK...**(또는 명령 팔레트의 **Load PDK...**)
- CLI: `ics load-pdk myproj pdks/sky130A.toml`
- MCP: `load_pdk` 도구

파일 안의 길이는 µm 단위입니다. 상대 경로는 PDK 파일 기준이고, `${VAR}`는 환경 변수로 확장되므로
모델 라이브러리가 설치된 PDK 안을 가리킬 수 있습니다.

```toml
[models]
path = "${PDK_ROOT}/sky130A/libs.tech/ngspice/sky130.lib.spice"
corners = ["tt", "ff", "ss", "sf", "fs"]
default = "tt"
length_unit = "um"   # 모델이 W / L을 µm로 받음
```

주요 섹션:

| 섹션 | 내용 |
|---|---|
| `name`, `dbu_per_um`, `grid` | 테크놀로지 이름, 데이터베이스 단위, 스냅 그리드(µm) |
| `lyp`, `lyt` | 레이어와 색상을 제공하는 KLayout 파일(선택) |
| `[[layer]]` | `name`, `gds`, `datatype`, `width`, `space`, `area`(µm²), `routing`, `color`(`#rrggbb`), `fill`, `stipple`, `outline`, `line`. 레이어를 추가하거나 `.lyp`에서 온 레이어에 룰을 설정 |
| `[roles]` | `diffusion`(리스트), `tap`, `poly`, `nwell`, `nimplant`(공정이 n+ 임플란트를 그리지 않으면 비움: n+는 `pimplant` 밖의 확산), `pimplant`, `diff_contact`, `poly_contact`(비아 이름), `label` |
| `[rules]` | `gate_contact_space`, `poly_endcap`, `implant_enc`, `nwell_enc`, `marker_enc`(확산 둘레의 소자 마커 레이어), `bend_corner`(꺾인 게이트의 모서리 스퀘어 중 W에 포함하는 비율, 0~1, 기본값 1. [PCell](pcells.md#추출이-인식하는-소자-형상) 참고) |
| `[[pin_label]]` | 도전 레이어의 넷 라벨 레이어: `layer`, `label`(예: `Metal1`과 `Metal1.text`). 라우팅은 이름 붙은 넷의 라벨을 여기에 두고, 추출은 여기의 라벨을 그 레이어 넷의 이름으로 읽음 |
| `[[pcell_layer]]` | PCell이 그리는 추가 레이어: `layer`, `around`(`poly_contact`, `diff_contact`, `gate`, `diffusion`, `poly` 또는 레이어 이름), `enc`, 선택적 `kinds`(`nmos`, `pmos`, `poly_res`, `mom_cap`, `ntap`, `ptap`) |
| `drc_extra` | 레이어 룰에서 유도한 덱 뒤에 덧붙일 덱 줄(최상위 키, 테이블보다 앞에 둠) |
| `[[via]]` | `name`, `bottom`, `cut`, `top`, `size`, `space`, `enc_bottom`, `enc_top`, `resistance`, 선택적 `em`(컷당 A) |
| `[[device]]` | `name`, `type`(`nmos` / `pmos`), `layers`, `exclude`, `subckt` |
| `[[resistor]]` | 저항: 도전 레이어의 마커로 표시된 본체: `name`, `layer`, `layers`(마커), `exclude`, `sheet`(스퀘어당 Ω). `poly_res`는 poly 레이어 저항의 마커를 그림 |
| `[[capacitor]]` | 커패시터: 두 도전 레이어의 마커로 표시된 겹침: `name`, `bottom`, `top`, `layers`, `area`(fF/µm²), 선택적 `perimeter`(fF/µm). `mom_cap`은 metal1 / metal2 커패시터의 마커를 그림 |
| `[models]` | `path`, `extra`(같은 코너로 함께 불러올 모델 파일), `corners`, `default`, `length_unit`(`m` 또는 `um`), `osdi`(ngspice가 먼저 불러오는 컴파일된 Verilog-A 모델, OpenVAF `.osdi`), `lde`(모델이 레이아웃 의존 파라미터 `nf sa sb sd`를 받으면 `true`. 이 경우 레이아웃 후 덱이 [이를 백어노테이션](simulation.md#포스트-레이아웃-시뮬레이션)) |
| `drc_deck` | DRC 덱 파일(선택) |
| `[[signoff]]` | 외부 DRC / LVS 런셋: `name`, `tool`(`klayout`, `calibre`, `netgen`, `builtin`), `check`(`drc`, `lvs`), `deck`, `vars`, `klayout`, `skip`, `ground`([검증](verification.md#외부-도구로-사인오프) 참고) |
| `[cross_section]` | 이미지센서 픽셀용 백엔드 단면([아래](#픽셀-단면) 참고) |
| `[[cap]]` | 기생 계수. `tech.json`과 같으며 선택적으로 `resistance`(스퀘어당 Ω)와 `em`(폭 µm당 A) |

PDK를 쓰면 달라지는 점:

- **추출**은 역할 레이어에서 게이트를 찾고, 각 트랜지스터에 같은 극성의 `[[device]]` 중 `layers`가
  모두 게이트를 덮고 `exclude` 레이어는 덮지 않는 첫 번째 소자의 이름을 붙입니다(예: 저 Vt 소자의
  `lvtn`).
- **넷리스트**는 회로도 모델 `nmos` / `pmos`를 극성별 첫 번째 소자에 매핑하고, `subckt` 소자는 `X`
  소자로 쓰며, Simulation 워크스페이스에서 고른 코너로 모델 라이브러리를 `.lib`로 불러옵니다. PDK
  모델에는 ngspice가 필요합니다.
- **LVS**는 소자 모델도 비교합니다.
- **PCell, 회로도 기반 레이아웃, 라우팅**은 역할 레이어, 비아, 룰을 사용합니다. 프로젝트에 이미
  있는 PCell 변형은 다시 생성됩니다.
- **MOS PCell**은 PDK의 어떤 `[[device]]`든 될 수 있으며, 그 경우 해당 마커 `layers`를 그립니다
  (예: 저 Vt 소자 둘레의 `lvtn`, 확산 너머 `marker_enc`). 회로도 기반 레이아웃은 회로도 모델로
  소자를 고르므로, 회로도의 저 Vt 트랜지스터는 그 소자로 배치되고 추출되고 비교됩니다.
  `[[pcell_layer]]`는 SKY130의 폴리 콘택 위 `npc` 같은 레이어를 추가합니다.

```toml
drc_extra = [
  "pcon = licon1 and poly",
  "pcon_npc = npc size -0.1",
  "pcon_bad = pcon not inside pcon_npc",
  "NPC.EN: flag pcon_bad",
]

[[pcell_layer]]
layer = "npc"
around = "poly_contact"
enc = 0.1
```

짧은 게이트 길이에서도 이웃한 소스 / 드레인 콘택이 첫 번째 메탈의 간격을 유지하도록, 필요하면
콘택과 게이트 사이 간격이 커집니다.

## 픽셀 단면

`[cross_section]`은 [픽셀 광학 모델](pixel-optics.md#pdk-단면에서-가져오기)을 위해 실리콘 위 백엔드를 기술합니다.
길이는 µm입니다.

```toml
[cross_section]
levels = [                                # 실리콘 표면부터 위로
  { name = "arc", material = "si3n4", thickness = 0.06, film = true },
  { name = "pmd", material = "sio2", thickness = 0.8 },
  { name = "metal 1", material = "sio2", thickness = 0.5, layer = "metal1", metal = "al" },
  { name = "imd 1", material = "sio2", thickness = 0.8 },
  { name = "metal 2", material = "sio2", thickness = 0.5, layer = "metal2", metal = "al" },
  { name = "passivation", material = "si3n4", thickness = 0.5 },
]
photodiode = { layer = "active not interact poly", top = 0.1, bottom = 3.0 }
pixel = { color_filter = 0.9, planarisation = 0.4, microlens_height = 2.5, center_on_photodiode = true }
silicon = { diffusion_length = 20.0, epi = 5.0 }
```

| 키 | 내용 |
|---|---|
| `levels` | `name`, `material`(유전체), `thickness`, 선택적으로 `layer`(레이아웃 레이어 또는 파생 레이어 식)와 `metal`(그 층 안의 금속 재료), `film`(코히어런트로 다루는 박막) |
| `photodiode` | `layer`(수집 영역의 레이어나 식), 실리콘 표면 아래 `top`, `bottom` 깊이 |
| `pixel` | 컬러 필터와 평탄화층 두께, 마이크로렌즈 높이, `center_on_photodiode` |
| `silicon` | 캐리어 수집용 소수 캐리어 확산 길이와 에피층 두께 |

## 실제 PDK 레이어 세트 가져오기

IC Studio는 KLayout 레이어 속성(`.lyp`)과, 선택적으로 KLayout 테크놀로지 파일(`.lyt`)을 읽습니다.

- GUI: **File > Import > KLayout layer properties...**
- CLI: `ics import-tech myproj sky130A.lyp --lyt sky130A.lyt`

가져오는 것:

- 구체적인 `layer/datatype` 소스가 있는 모든 레이어 항목이 레이어가 됩니다. 이름은 항목 이름에서
  가져옵니다(공백은 `_`, 중복은 `_2`, `_3` 등).
- 색상은 채우기 또는 테두리 색에서 가져옵니다(테두리 색이 다르면 외곽선 색이 됩니다). 디더
  패턴은 기본(`I0`–`I46`)이든 사용자 정의든 정확한 스티플 비트맵으로 보존됩니다. 선 스타일(기본
  또는 사용자 정의)과 테두리 폭이 외곽선 선 스타일을 정하며(폭 0이면 외곽선 없음), 빈 패턴의
  `xfill`은 X 채움이 됩니다.
- `poly`, `li`, `met1`, `metal2`, `m3` 같은 이름의 레이어는 배선 레이어로 표시됩니다.
- `.lyt`에서: 테크놀로지 이름, 데이터베이스 단위, 기본 그리드.

가져오지 않는 것: 폭 / 간격 룰, 비아, 기생 계수, DRC 덱. 그 테크놀로지로 DRC, 추출, 라우팅을
쓰려면 이것들을 `tech.json`에 추가하세요(덱은 **Rule deck** 편집기에서).

기존 레이아웃은 유지됩니다. 도형은 같은 GDS layer / datatype(또는 같은 이름)을 가진 새 레이어로
옮겨지고, 파일에 없지만 사용 중인 레이어는 유지되며, 데이터베이스 단위가 바뀌면 좌표가 다시
스케일됩니다.


## Virtuoso 테크놀로지 파일 가져오기

Cadence Virtuoso ASCII 테크놀로지 파일(`.tf`, **Tools > Technology File Manager > Dump**로 생성)을
디스플레이 리소스 파일과 스트림 레이어 맵과 함께 가져올 수 있습니다.

```sh
ics import-tech myproj mytech.tf --drf display.drf --layermap mytech.layermap
```

- 레이어: `techLayers`에 있는 레이어의 `techLayerPurposePriorities` 항목마다 하나. `drawing`
  퍼포스는 레이어 이름 그대로, 다른 퍼포스는 `layer.purpose`가 됩니다.
- 단위: µm당 데이터베이스 단위는 `viewTypeUnits`(`maskLayout`), 그리드는 `mfgGridResolution`.
- 룰: drawing 퍼포스의 단일 레이어 `minWidth`, `minSpacing`, `minArea`(`foundry` 제약 그룹 또는
  `physicalRules`).
- 표시: `.drf`에서 레이어-퍼포스의 `techDisplays` 패킷이 채움 색과 외곽선 색, 스티플 비트맵(그대로
  보존), 선 스타일(`drDefineLineStyle`), 채움 스타일(`outline`, `solid`, `X`, `stipple`,
  `outlineStipple`)을 정하므로 레이어가 Virtuoso에서와 같은 모습으로 보입니다.
- 스트림 번호: 레이어 맵에서 가져오며 맵은 테크놀로지의 스트림 맵이 됩니다. 맵이 없으면 레이어
  번호와 `drawing`의 datatype 0을 쓰고 추정임을 알립니다.

비아, 소자, 두 레이어 간 룰과 나머지 룰 세트는 읽지 않습니다. [PDK 파일](#pdk-파일)에 추가하세요.
