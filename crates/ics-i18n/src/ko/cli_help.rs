//! Korean help of the `ics` command line: command and argument help as clap shows it
//! (see `ics-cli/src/i18n.rs`).

pub const KO: &[(&str, &str)] = &[
    (
        "A dump file, or a cds.lib / a directory with one / a library directory: then Virtuoso (ICS_VIRTUOSO or `virtuoso` on the PATH) dumps the library in batch mode",
        "덤프 파일, 또는 cds.lib / 그것이 있는 디렉터리 / 라이브러리 디렉터리: 이 경우 Virtuoso(ICS_VIRTUOSO 또는 PATH의 `virtuoso`)가 배치 모드로 라이브러리를 덤프합니다",
    ),
    (
        "A target, `name>=value`, `name<=value` or `name=lo..hi` (repeatable). The name is a spec of the cell or a common one (gain_db, ugf, pm, bw, tphl, tplh); other specs keep their limits",
        "목표: `name>=value`, `name<=value` 또는 `name=lo..hi` (반복 가능). name은 셀의 스펙 또는 공통 스펙(gain_db, ugf, pm, bw, tphl, tplh)이며, 다른 스펙은 기존 한계를 유지합니다",
    ),
    (
        "Add a cell to the plan as a test (repeatable), save the project and exit",
        "셀을 계획에 테스트로 추가하고(반복 가능) 프로젝트를 저장한 뒤 종료합니다",
    ),
    (
        "Add layout parasitic capacitances (also enabled by the saved setup)",
        "레이아웃 기생 커패시턴스를 추가합니다 (저장된 설정으로도 켜짐)",
    ),
    (
        "Also draw the technology's measured process variation (`ics corners --apply`)",
        "공정 기술의 측정된 공정 변동도 샘플링합니다 (`ics corners --apply`)",
    ),
    (
        "Also print the image sensor pixels found around the photodiodes: TG, RST, SF, SEL with W / L and junctions, the floating diffusion and the photodiode geometry",
        "포토다이오드 주변에서 찾은 이미지 센서 픽셀도 출력합니다: W / L과 접합을 포함한 TG, RST, SF, SEL, 플로팅 디퓨전, 포토다이오드 형상",
    ),
    ("Also write the LEF abstracts of the placed cells to this file", "배치된 셀의 LEF 추상 뷰도 이 파일에 씁니다"),
    (
        "Analyse the column at this position (µm along the cut) as a pinned photodiode: the pinning voltage and storage capacitance of its buried n region",
        "이 위치(절단선을 따라 µm)의 열을 핀드 포토다이오드로 분석합니다: 매립 n 영역의 피닝 전압과 저장 커패시턴스",
    ),
    ("Analysis giving the x axis: dc, ac or tran", "x축을 주는 해석: dc, ac 또는 tran"),
    ("Another project directory to compare with instead of a revision", "리비전 대신 비교할 다른 프로젝트 디렉터리"),
    (
        "Apply verified quick fixes (top-level geometry only) and save the project first",
        "검증된 빠른 수정(최상위 형상만)을 먼저 적용하고 프로젝트를 저장합니다",
    ),
    ("Arguments", "인수"),
    ("Arguments as a JSON object", "JSON 객체로 된 인수"),
    ("Blur sigma in µm (default 0.45 x min width)", "블러 시그마 (µm, 기본값 0.45 x 최소 폭)"),
    ("Cell to create or replace (default: work/<top>)", "만들거나 교체할 셀 (기본값: work/<top>)"),
    (
        "Cell to write the schematic to (`lib/cell` or `cell`; created when missing)",
        "회로도를 쓸 셀 (`lib/cell` 또는 `cell`; 없으면 생성)",
    ),
    ("Change how a library is stored on disk", "라이브러리를 디스크에 저장하는 방식을 바꿉니다"),
    (
        "Check a schematic-driven layout against its schematic (missing, extra and changed devices, shorts, opens); `--update` places new devices and resizes changed ones, then saves. Non-zero exit when differences remain",
        "회로도 기반 레이아웃을 회로도와 비교합니다(누락·추가·변경된 소자, 단락, 개방). `--update`는 새 소자를 배치하고 변경된 소자의 크기를 바꾼 뒤 저장합니다. 차이가 남으면 0이 아닌 종료 코드",
    ),
    ("Check an extension folder's extension.toml without installing it", "설치하지 않고 확장 폴더의 extension.toml을 검사합니다"),
    ("Chief ray angle (degrees) and its azimuth from +x (degrees)", "주광선 각도(도)와 +x 기준 방위각(도)"),
    ("Colour temperature of the (blackbody) illuminant (K)", "(흑체) 광원의 색온도 (K)"),
    (
        "Column line capacitance (F) of the readout noise analysis of --characterize",
        "--characterize 판독 노이즈 해석의 열 라인 커패시턴스 (F)",
    ),
    ("Column of the x values (default: the first)", "x 값의 열 (기본값: 첫 번째)"),
    (
        "Compact a cell's layout toward the given directions (fixed items stay, constraints hold, no new DRC violations) and save the project",
        "셀의 레이아웃을 주어진 방향으로 압축하고(고정 항목 유지, 제약 조건 유지, 새 DRC 위반 없음) 프로젝트를 저장합니다",
    ),
    (
        "Compare a cell's layout geometry (flattened, XOR per layer) with a git revision of the project or with another project directory. Non-zero exit when they differ",
        "셀의 레이아웃 형상(평탄화, 레이어별 XOR)을 프로젝트의 git 리비전 또는 다른 프로젝트 디렉터리와 비교합니다. 다르면 0이 아닌 종료 코드",
    ),
    (
        "Compare a cell's layout with its schematic (non-zero exit on differences)",
        "셀의 레이아웃을 회로도와 비교합니다 (차이가 있으면 0이 아닌 종료 코드)",
    ),
    ("Compare every trace on a log scale", "모든 트레이스를 로그 스케일로 비교합니다"),
    ("Compare with this saved run instead of the latest one", "최신 실행 대신 이 저장된 실행과 비교합니다"),
    (
        "Copy a cell as a new cell (schematic, layout, simulation setup, specs and notes) to start a new design from a finished one, and save",
        "완성된 설계에서 새 설계를 시작하도록 셀(회로도, 레이아웃, 시뮬레이션 설정, 스펙, 메모)을 새 셀로 복사하고 저장합니다",
    ),
    (
        "Copy a deck file (IC Studio or SVRF) into the technology (unlinks a shared deck)",
        "덱 파일(IC Studio 또는 SVRF)을 공정 기술에 복사합니다 (공유 덱 연결 해제)",
    ),
    (
        "Corner models from silicon: fit model parameters so that the spec's simulated test structures reproduce the WAT data's means (tt) and its corners (ff, ss, fs, sf: primary parameters at ±zσ, the others following their correlation), and write a corner library",
        "실리콘 기반 코너 모델: 스펙의 시뮬레이션 테스트 구조가 WAT 데이터의 평균(tt)과 코너(ff, ss, fs, sf: 주 파라미터는 ±zσ, 나머지는 상관관계를 따름)를 재현하도록 모델 파라미터를 피팅하고 코너 라이브러리를 씁니다",
    ),
    ("Create a new project directory", "새 프로젝트 디렉터리를 만듭니다"),
    (
        "Create or replace a cell's schematic from a SPICE netlist file; its analysis lines set the cell's simulation setup. Saves the project",
        "SPICE 넷리스트 파일로 셀의 회로도를 만들거나 교체합니다. 해석 라인이 셀의 시뮬레이션 설정이 됩니다. 프로젝트를 저장합니다",
    ),
    (
        "Current factor mismatch coefficient per device in m (10n = 1 %·µm; a pair differs √2 times more)",
        "소자당 전류 계수 미스매치 계수 (m, 10n = 1 %·µm, 쌍의 차이는 √2배)",
    ),
    (
        "Cut line `x0,y0,x1,y1` in µm (default: horizontal through the middle of the layout)",
        "절단선 `x0,y0,x1,y1` (µm, 기본값: 레이아웃 중앙을 지나는 수평선)",
    ),
    (
        "DEF placements: export a top cell's components, pins and nets, import a DEF as a top cell",
        "DEF 배치: 최상위 셀의 컴포넌트, 핀, 넷을 내보내고 DEF를 최상위 셀로 가져옵니다",
    ),
    ("Delete an installed extension", "설치된 확장을 삭제합니다"),
    (
        "Derive a new design from a cell: copy it with new targets, size the suggested parameters (matched devices kept equal) with the optimizer, lay it out like the original, check it and save. Non-zero exit when a target is missed",
        "셀에서 새 설계를 파생합니다: 새 목표로 복사하고, 제안된 파라미터를 최적화기로 크기 조정하고(매칭 소자는 같게 유지), 원본처럼 레이아웃을 만들고, 검사한 뒤 저장합니다. 목표를 놓치면 0이 아닌 종료 코드",
    ),
    ("Directions in order: left, right, down, up", "순서대로 방향: left, right, down, up"),
    ("Do not build a layout", "레이아웃을 만들지 않습니다"),
    (
        "Do not infer symmetry constraints from the schematic's matched devices first",
        "먼저 회로도의 매칭 소자에서 대칭 제약 조건을 추론하지 않습니다",
    ),
    ("Do not save the project afterwards", "이후 프로젝트를 저장하지 않습니다"),
    ("Do not save the run in the history", "실행을 기록에 저장하지 않습니다"),
    (
        "Draw a schematic of the extraction (pixels in the textbook arrangement) with a test bench and transient setup, into the cell (or `--into`), and save the project",
        "추출 결과의 회로도(교과서식 배치의 픽셀)를 테스트벤치, 과도 해석 설정과 함께 셀(또는 `--into`)에 그리고 프로젝트를 저장합니다",
    ),
    ("Draw the devices only, without test bench sources and analyses", "테스트벤치 소스와 해석 없이 소자만 그립니다"),
    ("Emit results as JSON on stdout", "결과를 stdout에 JSON으로 출력합니다"),
    (
        "Evaluate every candidate after layout generation, routing and extraction",
        "레이아웃 생성, 배선, 추출 후 모든 후보를 평가합니다",
    ),
    (
        "Export a PCell Designer master as a SKILL pcDefinePCell for Cadence Virtuoso",
        "PCell Designer 마스터를 Cadence Virtuoso용 SKILL pcDefinePCell로 내보냅니다",
    ),
    (
        "Export a cell layout and every cell it places to GDSII, or to OASIS when the output file ends in `.oas` / `.oasis`",
        "셀 레이아웃과 그것이 배치한 모든 셀을 GDSII로, 출력 파일이 `.oas` / `.oasis`로 끝나면 OASIS로 내보냅니다",
    ),
    ("Extension id (default: the folder name)", "확장 id (기본값: 폴더 이름)"),
    (
        "Extract nets, transistors, resistors and capacitors from a cell layout and print them as SPICE. Photodiodes (the technology's cross-section photodiode region) are listed as comments",
        "셀 레이아웃에서 넷, 트랜지스터, 저항, 커패시터를 추출해 SPICE로 출력합니다. 포토다이오드(공정 기술 단면의 포토다이오드 영역)는 주석으로 나열됩니다",
    ),
    (
        "First place the schematic devices missing from the layout (schematic-driven layout)",
        "먼저 레이아웃에 없는 회로도 소자를 배치합니다 (회로도 기반 레이아웃)",
    ),
    ("Fit the parameters to the measurements", "파라미터를 측정값에 피팅합니다"),
    ("Furthest inward move in µm (default min width / 4)", "안쪽으로 최대 이동 거리 (µm, 기본값 최소 폭 / 4)"),
    ("Furthest outward move in µm (default min spacing / 3)", "바깥쪽으로 최대 이동 거리 (µm, 기본값 최소 간격 / 3)"),
    ("Git revision to compare with", "비교할 git 리비전"),
    ("Git tag or branch", "git 태그 또는 브랜치"),
    (
        "Global process variation (JSON from `ics corners --variation`). Without it, the technology's variation is used if it has one, else --vto-sigma / --kp-sigma",
        "전역 공정 변동 (`ics corners --variation`의 JSON). 없으면 공정 기술에 변동이 있으면 그것을, 아니면 --vto-sigma / --kp-sigma를 씁니다",
    ),
    ("Global relative kp sigma per polarity", "극성별 전역 상대 kp 시그마"),
    ("Global threshold sigma per polarity (V)", "극성별 전역 문턱 전압 시그마 (V)"),
    (
        "Harmonics per direction for RCWA (odd; more is more accurate and slower). Default: about 3.8 per µm of the Bayer cell, at least 11 (converged to about 0.01 in QE)",
        "RCWA의 방향별 고조파 수 (홀수; 많을수록 정확하고 느림). 기본값: Bayer 셀 µm당 약 3.8, 최소 11 (QE 기준 약 0.01로 수렴)",
    ),
    ("IC Studio command line interface", "IC Studio 명령줄 인터페이스"),
    (
        "IR drop and electromigration of one net of a cell layout (non-zero exit on EM violations). Pins labelled with the net name are the pads; device currents come from `--currents` or else from an operating-point simulation of the schematic",
        "셀 레이아웃의 한 넷의 IR 드롭과 일렉트로마이그레이션 (EM 위반 시 0이 아닌 종료 코드). 넷 이름이 레이블된 핀이 패드이며, 소자 전류는 `--currents` 또는 회로도의 동작점 시뮬레이션에서 가져옵니다",
    ),
    ("IR-cut filter edge (nm); 0 for none", "IR 차단 필터 경계 (nm); 없으면 0"),
    ("Ignore opens (unrouted nets) for the exit status", "종료 상태에서 개방(배선되지 않은 넷)을 무시합니다"),
    ("Illuminance at the pixel (lux) for the photocurrent", "광전류를 위한 픽셀의 조도 (lux)"),
    (
        "Image sensor pixel optics: quantum efficiency, crosstalk and losses of a pixel stack (TOML, see `pixels/`), and with `--lux` the photocurrent, optionally written into a current source of a pixel circuit (`--set PROJECT CELL INSTANCE`)",
        "이미지 센서 픽셀 광학: 픽셀 스택(TOML, `pixels/` 참고)의 양자 효율, 크로스토크, 손실, 그리고 `--lux`로 광전류를 계산하며, 선택적으로 픽셀 회로의 전류원에 씁니다 (`--set PROJECT CELL INSTANCE`)",
    ),
    (
        "Import a DEF file as a new top cell placing the project cells named by its components' macros, and save the project",
        "DEF 파일을 컴포넌트의 매크로 이름에 해당하는 프로젝트 셀을 배치하는 새 최상위 셀로 가져오고 프로젝트를 저장합니다",
    ),
    (
        "Import a GDSII or OASIS file as a new library stored as one GDS file. Layers the technology lacks are added to it",
        "GDSII 또는 OASIS 파일을 하나의 GDS 파일로 저장되는 새 라이브러리로 가져옵니다. 공정 기술에 없는 레이어는 추가됩니다",
    ),
    (
        "Import a Virtuoso library dump written by icsDumpLib: schematics (flattened, Virtuoso placement kept when possible) and layouts (with their masters). Saves the project",
        "icsDumpLib가 쓴 Virtuoso 라이브러리 덤프를 가져옵니다: 회로도(평탄화, 가능하면 Virtuoso 배치 유지)와 레이아웃(마스터 포함). 프로젝트를 저장합니다",
    ),
    (
        "Import a hierarchical CDL netlist (Virtuoso auCdl: .SUBCKT, *.PININFO, X ... / cell, subcircuit parameters, ! globals) as a flat schematic of one cell. Saves the project",
        "계층 CDL 넷리스트(Virtuoso auCdl: .SUBCKT, *.PININFO, X ... / cell, 서브서킷 파라미터, ! 전역 넷)를 한 셀의 평탄 회로도로 가져옵니다. 프로젝트를 저장합니다",
    ),
    (
        "Import every subcircuit as its own cell (each flattened from itself)",
        "모든 서브서킷을 각각의 셀로 가져옵니다 (각자 자기 자신에서 평탄화)",
    ),
    (
        "Import the macros of a LEF file as layout cells of a new library and save the project",
        "LEF 파일의 매크로를 새 라이브러리의 레이아웃 셀로 가져오고 프로젝트를 저장합니다",
    ),
    ("Install an extension again from where it came from", "확장을 원래 출처에서 다시 설치합니다"),
    (
        "Install from a folder, a git repository URL or an id of the extension index",
        "폴더, git 저장소 URL 또는 확장 인덱스의 id에서 설치합니다",
    ),
    (
        "JSON object of amps per layout device name (as `ics extract` prints them)",
        "레이아웃 소자 이름별 암페어의 JSON 객체 (`ics extract`가 출력하는 이름)",
    ),
    (
        "KLayout technology file (.lyt) supplying the name and database unit",
        "이름과 데이터베이스 단위를 제공하는 KLayout 공정 기술 파일 (.lyt)",
    ),
    (
        "Keep the hierarchy: a cell per subcircuit (in the library of --cell, default work), subcircuit instances as cell instances, a variant cell per other parameter values",
        "계층을 유지합니다: 서브서킷마다 셀 하나(--cell의 라이브러리, 기본값 work), 서브서킷 인스턴스는 셀 인스턴스로, 다른 파라미터 값마다 변형 셀 하나",
    ),
    (
        "LEF abstracts: export cells as LEF macros, import LEF macros as layout cells",
        "LEF 추상 뷰: 셀을 LEF 매크로로 내보내고 LEF 매크로를 레이아웃 셀로 가져옵니다",
    ),
    (
        "Language of messages and help: en or ko (default: ICS_LANG, else the system locale)",
        "메시지와 도움말의 언어: en 또는 ko (기본값: ICS_LANG, 없으면 시스템 로캘)",
    ),
    (
        "Largest relative error `--compare` accepts (non-zero exit above it)",
        "`--compare`가 허용하는 최대 상대 오차 (넘으면 0이 아닌 종료 코드)",
    ),
    ("Layer to correct, e.g. poly", "보정할 레이어 (예: poly)"),
    (
        "Let the supply current change freely (by default it stays within 0.5x to 2x of the original's, so a target is not met by starving or flooding the circuit)",
        "공급 전류를 자유롭게 바꿀 수 있게 합니다 (기본적으로 원본의 0.5x~2x 안에 머물러, 회로를 굶기거나 과하게 흘려서 목표를 맞추지 않게 합니다)",
    ),
    ("Library name (defaults to the file name; made unique)", "라이브러리 이름 (기본값: 파일 이름; 고유하게 만듦)"),
    ("Library of the new cell (default: the original's)", "새 셀의 라이브러리 (기본값: 원본의 라이브러리)"),
    (
        "Library of the new cell, searched first for macros (default `work`)",
        "새 셀의 라이브러리, 매크로를 먼저 찾는 곳 (기본값 `work`)",
    ),
    ("Library to import from a cds.lib", "cds.lib에서 가져올 라이브러리"),
    ("Link the technology to a shared deck file", "공정 기술을 공유 덱 파일에 연결합니다"),
    ("List libraries, cells and views of a project", "프로젝트의 라이브러리, 셀, 뷰를 나열합니다"),
    ("List the installed extensions (and folders that failed to load)", "설치된 확장(과 불러오지 못한 폴더)을 나열합니다"),
    ("List the tests and exit", "테스트를 나열하고 종료합니다"),
    ("Longest fragment in µm (default 2 x min width)", "가장 긴 조각 (µm, 기본값 2 x 최소 폭)"),
    ("Macro CLASS", "매크로 CLASS"),
    ("Manage extensions: install, list, create and run them", "확장 관리: 설치, 나열, 생성, 실행"),
    (
        "Manage the technology's DRC deck: show where it comes from, import or export a deck file, or link a shared deck file common to several projects (re-read on open, written when the deck is changed). Changes save the project",
        "공정 기술의 DRC 덱을 관리합니다: 출처를 표시하고, 덱 파일을 가져오거나 내보내거나, 여러 프로젝트에 공통인 공유 덱 파일에 연결합니다(열 때 다시 읽고 덱이 바뀌면 씀). 변경 시 프로젝트를 저장합니다",
    ),
    ("Matched-pair data (default: the synthetic generic 180 nm pairs)", "매칭 쌍 데이터 (기본값: 합성 generic 180 nm 쌍)"),
    (
        "Measured QE (CSV: a wavelength column in nm, and R / Gr / Gb / B or G columns as fractions or percent): compare the model with it (RMS difference per colour)",
        "측정 QE (CSV: nm 단위 파장 열과 비율 또는 퍼센트로 된 R / Gr / Gb / B 또는 G 열): 모델과 비교합니다 (색별 RMS 차이)",
    ),
    ("Measured data, CSV with a header row", "측정 데이터, 헤더 행이 있는 CSV"),
    (
        "Measurement `analysis:expression`, e.g. `ac:bw(v(out))` or `op:v(out)` (repeatable)",
        "측정 `analysis:expression`, 예: `ac:bw(v(out))` 또는 `op:v(out)` (반복 가능)",
    ),
    (
        "Mismatch from silicon: fit Pelgrom's law (σ² = A² / (W L) + σ0²) to matched-pair data (CSV: device, w, l in µm, dvt in V, dbeta relative) per device: Avt and Abeta with their standard errors and the measured against fitted σ of every size",
        "실리콘 기반 미스매치: 소자별로 매칭 쌍 데이터(CSV: device, µm 단위 w, l, V 단위 dvt, 상대값 dbeta)에 Pelgrom 법칙(σ² = A² / (W L) + σ0²)을 피팅합니다: 표준 오차를 포함한 Avt와 Abeta, 그리고 크기별 측정 σ 대 피팅 σ",
    ),
    (
        "Model runs the fit may use (with `--method rcwa`: the ray stage's; RCWA refines with a third of them)",
        "피팅이 쓸 수 있는 모델 실행 횟수 (`--method rcwa`에서는 광선 단계의 횟수; RCWA는 그 1/3로 정밀화)",
    ),
    (
        "Monte Carlo on the built-in engine: process shifts and Pelgrom mismatch of every MOS, with statistics of waveform-calculator measurements",
        "내장 엔진의 몬테카를로: 모든 MOS의 공정 변동과 Pelgrom 미스매치, 파형 계산기 측정값의 통계",
    ),
    ("Monte Carlo runs (overrides the setup's)", "몬테카를로 실행 횟수 (설정값을 덮어씀)"),
    ("Name of the new cell (default: <cell>_copy)", "새 셀의 이름 (기본값: <cell>_copy)"),
    ("Name of the new cell (default: <cell>_v2, _v3, ...)", "새 셀의 이름 (기본값: <cell>_v2, _v3, ...)"),
    (
        "Obstructions: merged shapes, per-layer blocks around the pins, or none",
        "장애물: 병합된 도형, 핀 주변의 레이어별 블록, 또는 none",
    ),
    ("Only print the suggested parameters and specs", "제안된 파라미터와 스펙만 출력합니다"),
    (
        "Optical proximity correction of one layer of a cell layout: a Gaussian-blur / threshold litho model and iterative edge-fragment correction. Prints the edge placement error without and with correction; `--apply` writes the mask to `<layer>_opc` (added to the technology on first use), replacing the previous mask, and saves the project. The drawn layer is never changed",
        "셀 레이아웃 한 레이어의 광 근접 보정: 가우시안 블러 / 문턱값 리소 모델과 반복적인 에지 조각 보정. 보정 전후의 에지 배치 오차를 출력합니다. `--apply`는 마스크를 `<layer>_opc`(처음 사용 시 공정 기술에 추가)에 써서 이전 마스크를 교체하고 프로젝트를 저장합니다. 그린 레이어는 절대 바뀌지 않습니다",
    ),
    ("Output file (.il)", "출력 파일 (.il)"),
    ("Output file (.il; default: standard output)", "출력 파일 (.il; 기본값: 표준 출력)"),
    ("Overwrite example projects that already exist", "이미 있는 예제 프로젝트를 덮어씁니다"),
    ("Pixel cell for a stack built from the technology's cross section", "공정 기술 단면으로 만든 스택의 픽셀 셀"),
    ("Pixel colour for `--set`: R, Gr, Gb or B", "`--set`의 픽셀 색: R, Gr, Gb 또는 B"),
    (
        "Place a cell's devices like another cell's layout with the same circuit topology (layout by example), optionally route it, and save",
        "같은 회로 구조를 가진 다른 셀의 레이아웃처럼 셀의 소자를 배치하고(예제 기반 레이아웃), 선택적으로 배선한 뒤 저장합니다",
    ),
    (
        "Positions along the cut (µm) to print the stack at (repeatable; default: 5 points)",
        "스택을 출력할 절단선 위 위치 (µm, 반복 가능; 기본값: 5개 지점)",
    ),
    ("Print help", "도움말을 출력합니다"),
    ("Print help (see a summary with '-h')", "도움말을 출력합니다 (요약은 '-h')"),
    ("Print help (see more with '--help')", "도움말을 출력합니다 (자세히는 '--help')"),
    ("Print the deck text", "덱 텍스트를 출력합니다"),
    (
        "Print the netlist of a cell (`lib/cell` or just `cell`): a SPICE deck, the same deck in Spectre, or CDL for LVS",
        "셀(`lib/cell` 또는 그냥 `cell`)의 넷리스트를 출력합니다: SPICE 덱, 같은 덱의 Spectre 형식, 또는 LVS용 CDL",
    ),
    ("Print the run as JSON", "실행을 JSON으로 출력합니다"),
    ("Print the stack built from the cross section (TOML) and stop", "단면으로 만든 스택(TOML)을 출력하고 멈춥니다"),
    ("Print this message or the help of the given subcommand(s)", "이 메시지 또는 지정한 하위 명령의 도움말을 출력합니다"),
    ("Print version", "버전을 출력합니다"),
    ("Print where the deck comes from and its rule count", "덱의 출처와 규칙 수를 출력합니다"),
    ("Print {output, value, modified} as JSON", "{output, value, modified}를 JSON으로 출력합니다"),
    (
        "Process data: WAT / PCM results (CSV with lot, wafer, site and parameter columns) against the process specification: capability (Cp, Cpk), out-of-spec sites, control chart rules on the wafer means, and whether the model corners cover the measured spread. Non-zero exit on out-of-spec sites or control-rule violations",
        "공정 데이터: 공정 사양 대비 WAT / PCM 결과(lot, wafer, site, 파라미터 열이 있는 CSV): 공정 능력(Cp, Cpk), 사양 이탈 사이트, 웨이퍼 평균에 대한 관리도 규칙, 모델 코너가 측정 산포를 덮는지 여부. 사양 이탈 사이트나 관리 규칙 위반 시 0이 아닌 종료 코드",
    ),
    (
        "Process emulation: the cross-section of a cell's layout along a cut line, built by running a process flow (TOML; default: the generic 180 nm flow) step by step. Prints the material stack at points along the cut; `--svg` writes the picture",
        "공정 에뮬레이션: 공정 플로우(TOML; 기본값: generic 180 nm 플로우)를 단계별로 실행해 만든, 절단선을 따른 셀 레이아웃의 단면. 절단선 위 지점의 재료 스택을 출력하며, `--svg`는 그림을 씁니다",
    ),
    ("Process flow file (see `process/generic180.toml`)", "공정 플로우 파일 (`process/generic180.toml` 참고)"),
    (
        "Project whose layout fills the stack's `layout` entries (`[layout] cell = ...`)",
        "스택의 `layout` 항목(`[layout] cell = ...`)을 채울 레이아웃의 프로젝트",
    ),
    (
        "Project whose technology models are the starting point (default: generic 180 nm)",
        "공정 기술 모델을 시작점으로 쓸 프로젝트 (기본값: generic 180 nm)",
    ),
    ("Project whose technology receives the coefficients with --apply", "--apply로 계수를 받을 공정 기술의 프로젝트"),
    ("Rays per pixel side (ray model)", "픽셀 한 변당 광선 수 (광선 모델)"),
    (
        "Read a GDSII or OASIS file and print hierarchy statistics and timings (nothing is written)",
        "GDSII 또는 OASIS 파일을 읽고 계층 통계와 소요 시간을 출력합니다 (아무것도 쓰지 않음)",
    ),
    ("Reference cell with a finished layout", "레이아웃이 완성된 기준 셀"),
    ("Replace an existing schematic", "기존 회로도를 교체합니다"),
    ("Replace an installed extension of the same id", "같은 id의 설치된 확장을 교체합니다"),
    ("Replace existing schematics and layouts", "기존 회로도와 레이아웃을 교체합니다"),
    (
        "Replace the technology with a PDK file (TOML: layers, vias, devices, models; see `pdks/sky130A.toml`). Variables like ${PDK_ROOT} in its paths come from the environment",
        "공정 기술을 PDK 파일(TOML: 레이어, 비아, 소자, 모델; `pdks/sky130A.toml` 참고)로 교체합니다. 경로의 ${PDK_ROOT} 같은 변수는 환경 변수에서 가져옵니다",
    ),
    (
        "Replace the technology with one read from KLayout layer properties (`.lyp`) or a Cadence Virtuoso ASCII technology file (`.tf`, with `--drf` and `--layermap`). Layout shapes keep their GDS layer/datatype; layers missing from the file are kept",
        "공정 기술을 KLayout 레이어 속성(`.lyp`) 또는 Cadence Virtuoso ASCII 공정 기술 파일(`.tf`, `--drf`와 `--layermap`과 함께)에서 읽은 것으로 교체합니다. 레이아웃 도형은 GDS layer/datatype을 유지하고, 파일에 없는 레이어는 유지됩니다",
    ),
    ("Resist threshold, 0..1 (default 0.5)", "레지스트 문턱값, 0..1 (기본값 0.5)"),
    (
        "Route a cell's flylines on the metal stack and save the project (non-zero exit when some nets stay unrouted)",
        "셀의 플라이라인을 메탈 스택에 배선하고 프로젝트를 저장합니다 (일부 넷이 배선되지 않으면 0이 아닌 종료 코드)",
    ),
    ("Rule deck file to use instead of the technology deck", "공정 기술 덱 대신 사용할 규칙 덱 파일"),
    (
        "Run DRC on a cell layout with the technology deck (or `--deck FILE`)",
        "공정 기술 덱(또는 `--deck FILE`)으로 셀 레이아웃에 DRC를 실행합니다",
    ),
    (
        "Run a Cadence SKILL (.il) script on the project and save it. Prints what the script printed; non-zero exit on a SKILL error (the project is then not saved)",
        "프로젝트에 Cadence SKILL(.il) 스크립트를 실행하고 저장합니다. 스크립트가 출력한 내용을 출력하며, SKILL 오류 시 0이 아닌 종료 코드 (이때 프로젝트는 저장하지 않음)",
    ),
    (
        "Run a tool (an extension's `<id>__<tool>` or a built-in one) on a project; saves the project when the tool changed it",
        "프로젝트에 도구(확장의 `<id>__<tool>` 또는 내장 도구)를 실행합니다. 도구가 프로젝트를 바꾸면 저장합니다",
    ),
    ("Run an extension's menu command on a cell of a project", "프로젝트의 셀에 확장의 메뉴 명령을 실행합니다"),
    (
        "Run an external sign-off DRC / LVS runset of the technology (KLayout, Calibre or netgen) on a cell; without a runset, list them. Non-zero exit on violations or a mismatch",
        "셀에 공정 기술의 외부 사인오프 DRC / LVS 런셋(KLayout, Calibre 또는 netgen)을 실행합니다. 런셋을 지정하지 않으면 나열합니다. 위반이나 불일치 시 0이 아닌 종료 코드",
    ),
    (
        "Run on both the built-in engine and ngspice and report how far they disagree",
        "내장 엔진과 ngspice 모두에서 실행하고 차이가 얼마인지 보고합니다",
    ),
    ("Run only these tests (names or cells; repeatable)", "이 테스트만 실행합니다 (이름 또는 셀; 반복 가능)"),
    (
        "Run the MCP server for AI agents on stdio (`claude mcp add ic-studio -- ics mcp`)",
        "AI 에이전트용 MCP 서버를 stdio로 실행합니다 (`claude mcp add ic-studio -- ics mcp`)",
    ),
    (
        "Run the cell's corner / Monte Carlo setup (`sim.sweep`) and predict the yield of each condition from a surrogate model of every spec, with the sensitivity of each spec to the varied parameters. Non-zero exit when a predicted yield is below `--target`",
        "셀의 코너 / 몬테카를로 설정(`sim.sweep`)을 실행하고 모든 스펙의 대리 모델로 각 조건의 수율을 예측합니다. 변동 파라미터에 대한 각 스펙의 민감도도 함께 출력합니다. 예측 수율이 `--target`보다 낮으면 0이 아닌 종료 코드",
    ),
    (
        "Run the project's verification plan (ADE Assembler style): every test's corner / sweep / Monte Carlo setup (`sim.sweep`), each spec summarised over its points (worst value, margin, pass count). Without a plan, every cell with specs is a test. The run is saved in results/verify/ and compared with the previous one. Non-zero exit on a failing or unmeasurable spec",
        "프로젝트의 검증 계획을 실행합니다(ADE Assembler 방식): 각 테스트의 코너 / 스윕 / 몬테카를로 설정(`sim.sweep`)을 실행하고, 각 스펙을 포인트 전체에 대해 요약합니다(최악값, 마진, 통과 수). 계획이 없으면 스펙이 있는 모든 셀이 테스트입니다. 실행은 results/verify/에 저장되고 이전 실행과 비교됩니다. 실패하거나 측정할 수 없는 스펙이 있으면 0이 아닌 종료 코드",
    ),
    ("Runset name (or a unique part of it)", "런셋 이름 (또는 고유한 일부)"),
    ("Save the setup given by the options with the cell", "옵션으로 지정한 설정을 셀에 저장합니다"),
    ("Search the extension index (all entries without a query)", "확장 인덱스를 검색합니다 (검색어가 없으면 모든 항목)"),
    (
        "Show the results of a sign-off run made elsewhere on a cell: KLayout `.lyrdb` / `.lvsdb`, Calibre ASCII results database or LVS report, netgen report. Non-zero exit on violations or an LVS mismatch",
        "다른 곳에서 실행한 셀의 사인오프 결과를 표시합니다: KLayout `.lyrdb` / `.lvsdb`, Calibre ASCII 결과 데이터베이스 또는 LVS 보고서, netgen 보고서. 위반이나 LVS 불일치 시 0이 아닌 종료 코드",
    ),
    (
        "Silicon-to-simulation correlation: compare measured curves (CSV) with the cell's simulation and, with `--fit`, fit model, instance or design parameters to them. The setup comes from the cell (`sim.silicon`); the options below replace parts of it",
        "실리콘-시뮬레이션 상관: 측정 곡선(CSV)을 셀의 시뮬레이션과 비교하고, `--fit`으로 모델, 인스턴스 또는 설계 파라미터를 피팅합니다. 설정은 셀(`sim.silicon`)에서 가져오며, 아래 옵션이 그 일부를 대체합니다",
    ),
    (
        "Simulate a SPICE or Spectre (`.scs`, `simulator lang=spectre`) file using its own analyses",
        "SPICE 또는 Spectre(`.scs`, `simulator lang=spectre`) 파일을 자체 해석으로 시뮬레이션합니다",
    ),
    ("Simulate a cell with its saved simulation setup", "저장된 시뮬레이션 설정으로 셀을 시뮬레이션합니다"),
    ("Simulation budget", "시뮬레이션 예산"),
    (
        "Simulator for --characterize (auto: ngspice when the PDK models need it)",
        "--characterize용 시뮬레이터 (auto: PDK 모델이 필요로 하면 ngspice)",
    ),
    (
        "Size the cell's devices to meet its specs (`sim.sweep.specs`) with the variables of its optimizer setup (`sim.optimize`); `--apply` writes the best values into the schematic",
        "최적화기 설정(`sim.optimize`)의 변수로 셀의 소자 크기를 스펙(`sim.sweep.specs`)에 맞게 조정합니다. `--apply`는 최적값을 회로도에 씁니다",
    ),
    (
        "Skip shapes on layers the technology does not define instead of adding the layers",
        "공정 기술에 정의되지 않은 레이어의 도형은 레이어를 추가하지 않고 건너뜁니다",
    ),
    (
        "Solver: `ray` (geometric rays, fast) or `rcwa` (rigorous coupled-wave analysis, includes diffraction; slower)",
        "솔버: `ray`(기하 광선, 빠름) 또는 `rcwa`(엄밀 결합파 해석, 회절 포함; 느림)",
    ),
    ("Specification (TOML; default: the generic 180 nm WAT spec)", "사양 (TOML; 기본값: generic 180 nm WAT 사양)"),
    (
        "Specification with `measure` per parameter and an `[extraction]` section (default: the generic 180 nm WAT spec)",
        "파라미터별 `measure`와 `[extraction]` 섹션이 있는 사양 (기본값: generic 180 nm WAT 사양)",
    ),
    (
        "Stack file; without one, the stack of `--cell` follows from the technology's cross section (`--project` and `--cell`)",
        "스택 파일. 없으면 `--cell`의 스택을 공정 기술 단면에서 만듭니다 (`--project`와 `--cell`)",
    ),
    ("Start a new extension in a folder", "폴더에 새 확장을 시작합니다"),
    ("Stop after this many steps", "이 단계 수 후에 멈춥니다"),
    (
        "Stop using the shared deck file; the project stores its deck itself",
        "공유 덱 파일 사용을 중단합니다. 프로젝트가 덱을 직접 저장합니다",
    ),
    (
        "Store the library as one JSON directory per cell instead of a GDS file",
        "라이브러리를 GDS 파일 대신 셀마다 JSON 디렉터리 하나로 저장합니다",
    ),
    (
        "Store the measured mismatch in the project's technology for Monte Carlo",
        "측정된 미스매치를 몬테카를로용으로 프로젝트의 공정 기술에 저장합니다",
    ),
    (
        "Store the process variation in the project's technology (needs --project); Monte Carlo then draws it in place of the generic sigmas",
        "공정 변동을 프로젝트의 공정 기술에 저장합니다 (--project 필요). 이후 몬테카를로는 일반 시그마 대신 이것을 씁니다",
    ),
    (
        "Stream layer map (Virtuoso layer map file) for the GDSII / OASIS numbers",
        "GDSII / OASIS 번호용 스트림 레이어 맵 (Virtuoso 레이어 맵 파일)",
    ),
    ("Sub-folder of the repository holding extension.toml", "extension.toml이 있는 저장소의 하위 폴더"),
    ("Subcircuit to import (default: the top one)", "가져올 서브서킷 (기본값: 최상위 서브서킷)"),
    ("Surrogate samples per condition", "조건별 대리 모델 샘플 수"),
    (
        "Symbol map (TOML [[symbol]] entries: kind, model, lib, cell, terms, params)",
        "심볼 맵 (TOML [[symbol]] 항목: kind, model, lib, cell, terms, params)",
    ),
    ("Symbol map (TOML [[symbol]] entries; default: analogLib)", "심볼 맵 (TOML [[symbol]] 항목; 기본값: analogLib)"),
    ("Test bench photocurrent per photodiode (A)", "포토다이오드별 테스트벤치 광전류 (A)"),
    ("Test bench supply and gate high level (V)", "테스트벤치 전원 및 게이트 하이 레벨 (V)"),
    (
        "Test bench temperature (°C): the simulations and the photodiode dark current",
        "테스트벤치 온도 (°C): 시뮬레이션과 포토다이오드 암전류에 적용",
    ),
    ("The cell `${cell}` stands for", "`${cell}`이 가리키는 셀"),
    ("The master cell (made with the PCell Designer)", "마스터 셀 (PCell Designer로 만든 것)"),
    (
        "Threshold mismatch coefficient per device in V·m (5n = 5 mV·µm; a pair differs √2 times more)",
        "소자당 문턱 전압 미스매치 계수 (V·m, 5n = 5 mV·µm, 쌍의 차이는 √2배)",
    ),
    (
        "Time (s) between the reset and the signal sample of correlated double sampling",
        "상관 이중 샘플링의 리셋과 신호 샘플 사이 시간 (s)",
    ),
    ("Update the layout from the schematic and save the project", "회로도에서 레이아웃을 갱신하고 프로젝트를 저장합니다"),
    ("Usage:", "사용법:"),
    (
        "Use the generic sigmas and Avt / Abeta even when the technology has a measured variation or mismatch",
        "공정 기술에 측정된 변동이나 미스매치가 있어도 일반 시그마와 Avt / Abeta를 사용합니다",
    ),
    (
        "Virtuoso display resource file (display.drf) for colours and fill patterns",
        "색과 채우기 패턴용 Virtuoso 디스플레이 리소스 파일 (display.drf)",
    ),
    (
        "Virtuoso layer map file (`RX drawing 2 0` per line) applied before reading: its rows join the technology's stream map and layers it names that the technology lacks are added",
        "읽기 전에 적용할 Virtuoso 레이어 맵 파일 (줄마다 `RX drawing 2 0`): 그 행이 공정 기술의 스트림 맵에 합쳐지고, 공정 기술에 없는 레이어는 추가됩니다",
    ),
    (
        "Virtuoso library of the PCell (default: the master's library)",
        "PCell의 Virtuoso 라이브러리 (기본값: 마스터의 라이브러리)",
    ),
    (
        "Virtuoso library to write the schematic into (default: the cell's library)",
        "회로도를 쓸 Virtuoso 라이브러리 (기본값: 셀의 라이브러리)",
    ),
    ("Virtuoso user units per schematic grid point", "회로도 그리드 점당 Virtuoso 사용자 단위"),
    ("WAT / PCM data (CSV)", "WAT / PCM 데이터 (CSV)"),
    ("Wavelength range and step (nm)", "파장 범위와 간격 (nm)"),
    (
        "With --fit: write the stack file with the fitted values (comments kept)",
        "--fit과 함께: 피팅된 값으로 스택 파일을 씁니다 (주석 유지)",
    ),
    (
        "With --measured: fit these stack parameters to it (comma separated): cf_red, cf_green, cf_blue, cf (all three densities), cf_density (one factor for all), cf_thickness, arc, si_thickness, lens, lens_n / lens_alpha / lens_shape (a superellipse lens's exponents), qe_scale",
        "--measured와 함께: 이 스택 파라미터를 측정값에 피팅합니다 (쉼표로 구분): cf_red, cf_green, cf_blue, cf (세 밀도 모두), cf_density (모두에 한 계수), cf_thickness, arc, si_thickness, lens, lens_n / lens_alpha / lens_shape (초타원 렌즈의 지수), qe_scale",
    ),
    (
        "With --measured: write the measured QE and the model before / after the fit per wavelength (CSV)",
        "--measured와 함께: 파장별 측정 QE와 피팅 전후 모델을 씁니다 (CSV)",
    ),
    (
        "With --pinned-pd: write them into the technology's photodiode (`pinning`, `cap_area`) and save the project",
        "--pinned-pd와 함께: 공정 기술의 포토다이오드(`pinning`, `cap_area`)에 쓰고 프로젝트를 저장합니다",
    ),
    (
        "With --pixel: simulate each pixel (up to 16) with charge injection for its FD and photodiode capacitance, conversion gain (µV/e-) and source follower gain",
        "--pixel과 함께: 각 픽셀(최대 16개)을 전하 주입으로 시뮬레이션해 FD와 포토다이오드 커패시턴스, 변환 이득(µV/e-), 소스 팔로워 이득을 구합니다",
    ),
    (
        "With --update: also delete layout devices that are no longer in the schematic",
        "--update와 함께: 회로도에 더 이상 없는 레이아웃 소자도 삭제합니다",
    ),
    (
        "With a `[device]` model and a stack read from a layout: write the photodiodes' mean pinning voltage and storage capacitance into the technology's photodiode (the pinned photodiode of the circuit simulation, `ics extract --pixel --characterize`) and save the project",
        "`[device]` 모델과 레이아웃에서 읽은 스택이 있을 때: 포토다이오드의 평균 피닝 전압과 저장 커패시턴스를 공정 기술의 포토다이오드(회로 시뮬레이션의 핀드 포토다이오드, `ics extract --pixel --characterize`)에 쓰고 프로젝트를 저장합니다",
    ),
    (
        "With a `[device]` model: simulate the charge transfer of this pixel (R, Gr, Gb, B), or `none` to skip the one the stack asks for",
        "`[device]` 모델이 있을 때: 이 픽셀(R, Gr, Gb, B)의 전하 전송을 시뮬레이션합니다. `none`이면 스택이 요청한 픽셀을 건너뜁니다",
    ),
    ("Write JUnit XML (for CI test reports)", "JUnit XML을 씁니다 (CI 테스트 보고서용)"),
    (
        "Write a cell's schematic as a SKILL script that recreates it in Virtuoso (load it in the CIW), with analogLib symbols or the symbols of a map file",
        "셀의 회로도를 Virtuoso에서 다시 만드는 SKILL 스크립트로 씁니다(CIW에서 load). analogLib 심볼 또는 맵 파일의 심볼을 사용합니다",
    ),
    (
        "Write a top cell as DEF (die area, components, pins, nets); to stdout without `--out`",
        "최상위 셀을 DEF(다이 영역, 컴포넌트, 핀, 넷)로 씁니다. `--out`이 없으면 stdout으로",
    ),
    (
        "Write fitted instance parameters into the schematic; also saves the setup",
        "피팅된 인스턴스 파라미터를 회로도에 씁니다. 설정도 저장합니다",
    ),
    (
        "Write icsDump.il, the SKILL script that dumps a Virtuoso library for import-virtuoso (in the CIW: load(\"icsDump.il\") then icsDumpLib(\"myLib\" \"/tmp/myLib.icsdump\"))",
        "import-virtuoso용으로 Virtuoso 라이브러리를 덤프하는 SKILL 스크립트 icsDump.il을 씁니다 (CIW에서: load(\"icsDump.il\") 후 icsDumpLib(\"myLib\" \"/tmp/myLib.icsdump\"))",
    ),
    (
        "Write the abstracts of cells (pins from labelled nets, OBS from the other metal) with a technology header; to stdout without `--out`",
        "셀의 추상 뷰(레이블된 넷에서 핀, 나머지 메탈에서 OBS)를 공정 기술 헤더와 함께 씁니다. `--out`이 없으면 stdout으로",
    ),
    (
        "Write the corner library (`.lib tt` ... sections, and `.lib mc`) to this file",
        "코너 라이브러리(`.lib tt` ... 섹션과 `.lib mc`)를 이 파일에 씁니다",
    ),
    ("Write the cross-section as SVG", "단면을 SVG로 씁니다"),
    (
        "Write the dopant depth profile at the first --at position (or the middle) as CSV",
        "첫 번째 --at 위치(또는 중앙)의 도펀트 깊이 프로파일을 CSV로 씁니다",
    ),
    (
        "Write the example projects, one per topic, to DIR/<topic> (existing ones are kept unless `--force`); without DIR, list the topics",
        "주제별 예제 프로젝트를 DIR/<topic>에 씁니다 (`--force`가 없으면 기존 것은 유지). DIR이 없으면 주제를 나열합니다",
    ),
    ("Write the fitted model cards to this file", "피팅된 모델 카드를 이 파일에 씁니다"),
    (
        "Write the global process variation (JSON, for `ics mc --variation`) to this file",
        "전역 공정 변동(JSON, `ics mc --variation`용)을 이 파일에 씁니다",
    ),
    ("Write the mask to `<layer>_opc` and save the project", "마스크를 `<layer>_opc`에 쓰고 프로젝트를 저장합니다"),
    (
        "Write the photocurrent of `--color` into the DC value of a current source: PROJECT CELL INSTANCE. Saves the project",
        "`--color`의 광전류를 전류원의 DC 값에 씁니다: PROJECT CELL INSTANCE. 프로젝트를 저장합니다",
    ),
    ("Write the technology's deck to a file", "공정 기술의 덱을 파일에 씁니다"),
    ("Yield (0 to 1) every condition must reach", "모든 조건이 도달해야 하는 수율 (0~1)"),
    ("`.lyp` (KLayout) or `.tf` (Virtuoso) file", "`.lyp` (KLayout) 또는 `.tf` (Virtuoso) 파일"),
    ("`<extension id>/<command id>`", "`<extension id>/<command id>`"),
    (
        "`COLUMN=EXPR[@BIAS]`, e.g. `id_vg0.9=-i(vd)@VG.dc=0.9` (repeatable)",
        "`COLUMN=EXPR[@BIAS]`, 예: `id_vg0.9=-i(vd)@VG.dc=0.9` (반복 가능)",
    ),
    (
        "`TARGET=MIN:MAX[:log]`, e.g. `model:nmos.vto=0.3:0.7` or `M1.w=1u:4u` (repeatable)",
        "`TARGET=MIN:MAX[:log]`, 예: `model:nmos.vto=0.3:0.7` 또는 `M1.w=1u:4u` (반복 가능)",
    ),
];
