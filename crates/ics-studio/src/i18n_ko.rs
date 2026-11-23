//! Korean UI strings, keyed by the English source text (see `i18n`). Hangul needs a system
//! font, so this file is not part of the bundled-font glyph check.

/// Whole UI strings.
pub const KO: &[(&str, &str)] = &[
    ("File", "파일"),
    ("New Project…", "새 프로젝트…"),
    ("New Demo Project", "새 데모 프로젝트"),
    ("Open Example", "예제 열기"),
    ("Write the example projects, one folder per topic", "예제 프로젝트를 주제별 폴더로 저장합니다"),
    ("Open Project…", "프로젝트 열기…"),
    ("Open Recent", "최근 프로젝트"),
    ("No recent projects", "최근 프로젝트 없음"),
    ("Save", "저장"),
    ("Save As…", "다른 이름으로 저장…"),
    ("Check and Save", "검사 후 저장"),
    ("Check the active schematic, save if it has no errors", "현재 회로도를 검사하고, 오류가 없으면 저장합니다"),
    ("New Cell…", "새 셀…"),
    ("Copy Cell as New…", "셀을 새 셀로 복사…"),
    (
        "Start a new design from the active cell: every view, specs and notes",
        "현재 셀에서 새 설계를 시작합니다: 모든 뷰, 스펙, 노트를 복사",
    ),
    ("Import", "가져오기"),
    ("GDSII / OASIS…", "GDSII / OASIS…"),
    ("PDK…", "PDK…"),
    ("GDSII / OASIS with Layer Map…", "레이어 맵으로 GDSII / OASIS…"),
    ("LEF abstracts…", "LEF 추상…"),
    ("DEF placement…", "DEF 배치…"),
    ("KLayout layer properties…", "KLayout 레이어 속성…"),
    ("CDL netlist (Virtuoso)…", "CDL 넷리스트 (Virtuoso)…"),
    ("Virtuoso library (dump or cds.lib)…", "Virtuoso 라이브러리 (덤프 또는 cds.lib)…"),
    ("Export", "내보내기"),
    ("SPICE netlist…", "SPICE 넷리스트…"),
    ("Cell GDSII…", "셀 GDSII…"),
    ("Library GDSII…", "라이브러리 GDSII…"),
    ("Virtuoso schematic (SKILL)…", "Virtuoso 회로도 (SKILL)…"),
    ("Virtuoso dump script (icsDump.il)…", "Virtuoso 덤프 스크립트 (icsDump.il)…"),
    ("Cell OASIS…", "셀 OASIS…"),
    ("Library OASIS…", "라이브러리 OASIS…"),
    ("Cell LEF abstract…", "셀 LEF 추상…"),
    ("Cell DEF…", "셀 DEF…"),
    ("Close Project", "프로젝트 닫기"),
    ("Settings…", "설정…"),
    ("Quit", "종료"),
    ("Edit", "편집"),
    ("Undo", "실행 취소"),
    ("Redo", "다시 실행"),
    ("Delete", "삭제"),
    ("View", "보기"),
    ("Fit to Window", "창에 맞추기"),
    ("Toggle Console", "콘솔 표시/숨기기"),
    ("Create", "만들기"),
    ("Simulation", "시뮬레이션"),
    ("Run", "실행"),
    ("Waveform Calculator…", "파형 계산기…"),
    ("Results Browser…", "결과 브라우저…"),
    ("Pixel Optics…", "픽셀 광학…"),
    ("Process Data…", "공정 데이터…"),
    ("Verify", "검증"),
    ("Mark Net", "넷 표시"),
    ("Tools", "도구"),
    ("Design Copilot…", "설계 Copilot…"),
    ("AI Models…", "AI 모델…"),
    ("Layout Generators", "레이아웃 생성기"),
    ("DRC Rule Designer…", "DRC 규칙 디자이너…"),
    ("DRC Deck", "DRC 데크"),
    ("SKILL CIW", "SKILL CIW"),
    ("Options", "옵션"),
    ("Help", "도움말"),
    ("Interactive Tutorial", "대화형 튜토리얼"),
    (
        "Show the everyday design flow in the menus and hide specialist tools (the command palette still finds every command)",
        "메뉴에는 일상적인 설계 흐름만 보이고 전문 도구는 숨깁니다 (명령 팔레트에서는 모든 명령을 찾을 수 있습니다)",
    ),
    ("User Guide", "사용자 가이드"),
    ("Keyboard Shortcuts", "단축키"),
    ("Command Palette", "명령 팔레트"),
    ("Check for Updates…", "업데이트 확인…"),
    ("About IC Studio", "IC Studio 정보"),
    ("New project", "새 프로젝트"),
    ("Name", "이름"),
    ("Location", "위치"),
    ("…", "…"),
    ("Template", "템플릿"),
    ("Empty", "빈 프로젝트"),
    ("Demo designs", "데모 설계"),
    ("Will be created at {}", "{}에 만들어집니다"),
    ("Cancel", "취소"),
    ("New cell", "새 셀"),
    ("Library", "라이브러리"),
    ("Cell name", "셀 이름"),
    ("Description", "설명"),
    ("Views", "뷰"),
    ("schematic", "회로도"),
    ("layout", "레이아웃"),
    ("simulation", "시뮬레이션"),
    ("Rename cell", "셀 이름 바꾸기"),
    ("Rename", "이름 바꾸기"),
    ("Copy as new cell", "새 셀로 복사"),
    (
        "Start a new design from {}: the schematic, layout, simulation setup, specs and notes are copied.",
        "{}에서 새 설계를 시작합니다: 회로도, 레이아웃, 시뮬레이션 설정, 스펙, 노트가 복사됩니다.",
    ),
    ("New name", "새 이름"),
    ("Copy", "복사"),
    ("IC Studio", "IC Studio"),
    (
        "Full-custom IC design — schematic, simulation and layout in one place.",
        "풀커스텀 IC 설계 — 회로도, 시뮬레이션, 레이아웃을 한곳에서.",
    ),
    ("Start", "시작"),
    ("Take the tutorial", "튜토리얼 시작"),
    ("New here? 10 guided steps", "처음이신가요? 10단계 안내"),
    ("Open demo project", "데모 프로젝트 열기"),
    ("Auto P&R, OTA, SRAM, pixels", "자동 P&R, OTA, SRAM, 픽셀"),
    ("New project…", "새 프로젝트…"),
    ("Empty, generic 180 nm PDK", "빈 프로젝트, 범용 180 nm PDK"),
    ("Open project…", "프로젝트 열기…"),
    ("Browse for a project folder", "프로젝트 폴더 찾아보기"),
    ("Examples", "예제"),
    ("Recent projects", "최근 프로젝트"),
    ("Workflow", "작업 흐름"),
    ("not saved yet", "아직 저장 안 됨"),
    ("+ New cell", "+ 새 셀"),
    ("⬆ Import GDS / OASIS…", "⬆ GDS / OASIS 가져오기…"),
    ("⬇ Export library GDS…", "⬇ 라이브러리 GDS 내보내기…"),
    ("Tutorial", "튜토리얼"),
    ("Learn the design flow step by step", "설계 흐름을 단계별로 배웁니다"),
    ("Library · {} ({} cells)", "라이브러리 · {} (셀 {}개)"),
    ("No cells yet. Create one to start designing.", "아직 셀이 없습니다. 셀을 만들어 설계를 시작하세요."),
    ("measured only", "측정만"),
    ("Specs (targets)", "스펙 (목표)"),
    (
        "No specs yet. Specs say what the design must achieve, e.g. gain >= 40 dB; they are checked on every run.",
        "아직 스펙이 없습니다. 스펙은 설계가 달성해야 할 목표(예: 이득 >= 40 dB)이며, 실행할 때마다 검사됩니다.",
    ),
    ("{} on the {} analysis", "{} ({} 해석)"),
    ("Edit specs…", "스펙 편집…"),
    ("Corner / Sweep Manager", "코너 / 스윕 관리자"),
    ("Check specs", "스펙 검사"),
    ("Run every spec over its corners (Verification)", "모든 스펙을 코너별로 검사합니다 (Verification)"),
    ("Derive a new design from this…", "이 설계에서 새 설계 파생…"),
    (
        "Copy this cell with its specs and notes, set new targets and let IC Studio size and lay it out",
        "이 셀을 스펙·노트와 함께 복사하고, 새 목표를 정하면 IC Studio가 크기를 정하고 레이아웃까지 만듭니다",
    ),
    ("Design notes", "설계 노트"),
    (
        "What is this cell for, why is it sized this way, what would you change for another target?",
        "이 셀은 무엇을 위한 것인지, 왜 이 크기인지, 다른 목표라면 무엇을 바꿀지 적어 두세요.",
    ),
    ("Notes", "노트"),
    ("{} of {} ({})", "{} — {} ({})"),
    ("Derive New Design", "새 설계 파생"),
    ("Sizing {}…", "{} 크기 조정 중…"),
    ("simulation {} of up to {}", "시뮬레이션 {} / 최대 {}"),
    (
        "The optimizer tries sizes and simulates each, keeping the matched devices equal.",
        "최적화기가 여러 크기를 시도하며 각각 시뮬레이션합니다. 정합 소자는 같은 크기로 유지됩니다.",
    ),
    ("Stop", "중지"),
    (
        "Start a new design from {}. The original stays as it is; the copy gets your targets, is sized to meet them and laid out like the original.",
        "{}에서 새 설계를 시작합니다. 원본은 그대로 두고, 복사본에 새 목표를 적용해 그 목표에 맞게 크기를 정한 뒤 원본처럼 레이아웃합니다.",
    ),
    ("1. Name", "1. 이름"),
    ("in {}", "{}에"),
    ("2. Targets", "2. 목표"),
    (
        "What the new design must achieve. Edit the limits (SPICE numbers: 40, 100meg, 50p); leave both empty to only measure.",
        "새 설계가 달성해야 할 목표입니다. 한계값을 고치세요(SPICE 숫자: 40, 100meg, 50p). 둘 다 비우면 측정만 합니다.",
    ),
    ("Use this spec", "이 스펙 사용"),
    (
        "These are the original's targets: the copy would only be re-sized for more margin. Change a limit to design for a new target.",
        "원본과 같은 목표입니다: 복사본은 여유(margin)를 늘리는 쪽으로만 다시 조정됩니다. 새 목표로 설계하려면 한계값을 바꾸세요.",
    ),
    ("Add:", "추가:"),
    ("3. What IC Studio may change", "3. IC Studio가 바꿀 수 있는 것"),
    (
        "Suggested from the circuit. Matched transistors share one value so they stay equal.",
        "회로에서 찾아 제안한 항목입니다. 정합 트랜지스터는 하나의 값을 공유해 같은 크기로 유지됩니다.",
    ),
    ("No transistor sizes found to change.", "바꿀 트랜지스터 크기를 찾지 못했습니다."),
    ("4. Options", "4. 옵션"),
    ("Build the layout like the original", "원본처럼 레이아웃 만들기"),
    (
        "Place like the original (or generate from the schematic if the new sizes do not fit), route, fix DRC, run LVS",
        "원본처럼 배치(새 크기가 맞지 않으면 회로도에서 생성)하고, 배선, DRC 수정, LVS 실행까지 합니다",
    ),
    ("Simulations at most", "최대 시뮬레이션 수"),
    ("Create and size", "만들고 크기 정하기"),
    ("{} meets every target.", "{}이(가) 모든 목표를 만족합니다."),
    ("{} does not meet every target yet.", "{}이(가) 아직 모든 목표를 만족하지 못합니다."),
    ("{} simulation(s); the original {} is unchanged.", "시뮬레이션 {}회. 원본 {}은(는) 그대로입니다."),
    ("New sizes", "새 크기"),
    ("Layout", "레이아웃"),
    ("Derive another", "하나 더 파생"),
    ("Close", "닫기"),
    ("Complete", "완료"),
    ("Step {} of {}", "{} / {} 단계"),
    ("{} of {} done", "{} / {} 완료"),
    ("You're ready to design", "이제 설계할 준비가 되었습니다"),
    ("Keyboard shortcuts", "단축키"),
    ("User guide", "사용자 가이드"),
    ("Start over", "처음부터"),
    ("Try", "해 보기"),
    ("Done", "완료"),
    ("Waiting for you to do it", "직접 해 보기를 기다리는 중"),
    ("Back", "이전"),
    ("Show me", "보여 주기"),
    ("Do this step for me", "이 단계를 대신 해 줍니다"),
    ("Do this step yourself, or skip it", "이 단계는 직접 하거나 건너뛰세요"),
    ("These steps use the demo project's cmos_inverter cell.", "이 단계들은 데모 프로젝트의 cmos_inverter 셀을 사용합니다."),
    ("All steps", "모든 단계"),
    ("What do W, L and M do? (beginner course)", "W, L, M은 무엇을 하나요? (입문 과정)"),
    ("{} in saturation · {} in triode · {} off", "포화 {} · 트라이오드 {} · 꺼짐 {}"),
    (
        "Saturation: the transistor works as a current source (amplifiers, loads, mirrors). Triode: it works as a resistor or closed switch (logic, switches). Off: no current. Hover a device's region for advice.",
        "포화: 트랜지스터가 전류원처럼 동작합니다(증폭기, 부하, 미러). 트라이오드: 저항이나 닫힌 스위치처럼 동작합니다(로직, 스위치). 꺼짐: 전류가 흐르지 않습니다. 소자의 영역에 마우스를 올리면 조언이 나옵니다.",
    ),
    ("{} / {} (worst at {})", "{} / {} (최악: {})"),
    ("What to try", "해 볼 것"),
    ("Explain how {} works", "{}의 동작 설명"),
    (
        "Explain in simple terms how {} works and what each device does.",
        "{}이(가) 어떻게 동작하는지, 각 소자가 무슨 역할을 하는지 쉽게 설명해 줘.",
    ),
    ("Derive a new design from {}", "{}에서 새 설계 파생"),
    (
        "Start a new design from {}: copy it, ask me for the new targets, size the copy to meet them and build its layout like the original.",
        "{}에서 새 설계를 시작해 줘: 복사하고, 새 목표를 나에게 물어본 뒤, 그 목표에 맞게 크기를 정하고 원본처럼 레이아웃을 만들어 줘.",
    ),
    ("Why does {} miss its specs?", "{}이(가) 스펙을 못 맞추는 이유는?"),
    (
        "Simulate {}, check its specs and explain which ones fail, why, and what I should change.",
        "{}을(를) 시뮬레이션하고 스펙을 검사해서, 어떤 스펙이 왜 실패하는지, 무엇을 바꿔야 하는지 설명해 줘.",
    ),
    ("Check the operating points", "동작점 확인"),
    (
        "Simulate {} and tell me whether every transistor is in the region it should be in, and why.",
        "{}을(를) 시뮬레이션하고, 모든 트랜지스터가 있어야 할 영역에 있는지와 그 이유를 알려 줘.",
    ),
    ("Which example should I start from?", "어떤 예제에서 시작할까요?"),
    (
        "I am new to IC design. Which example cell should I start from for an amplifier, a logic gate or a reference, and why?",
        "IC 설계가 처음이야. 증폭기, 로직 게이트, 기준 전압을 만들려면 각각 어떤 예제 셀에서 시작하면 좋은지, 그 이유도 알려 줘.",
    ),
    ("How do I start a new design?", "새 설계는 어떻게 시작하나요?"),
    (
        "How do I start a new design from one of the examples in IC Studio?",
        "IC Studio에서 예제를 바탕으로 새 설계를 시작하려면 어떻게 해?",
    ),
    (
        "Ask for a design task, e.g. \"place and route nand2, then fix DRC and run LVS\" or \"size the OTA for 60 dB gain and check the phase margin\", or ask how to use IC Studio, e.g. \"how do I import a GDS file?\": answers come from the user manual. Turn on Plan mode to review a plan before anything is edited.",
        "설계 작업을 요청하세요. 예: \"nand2를 배치·배선하고 DRC를 고친 뒤 LVS를 돌려 줘\", \"OTA를 이득 60 dB로 맞추고 위상 여유를 확인해 줘\". IC Studio 사용법도 물어볼 수 있습니다. 예: \"GDS 파일은 어떻게 가져와?\" — 답은 사용자 매뉴얼에서 찾습니다. Plan mode를 켜면 편집 전에 계획을 먼저 검토할 수 있습니다.",
    ),
    ("Try:", "해 보기:"),
    ("Beginner Mode", "초심자 모드"),
    ("Open {}", "{} 열기"),
    ("Create {} view", "{} 뷰 만들기"),
    ("Run Simulation", "시뮬레이션 실행"),
    ("Place in Active Schematic", "현재 회로도에 배치"),
    ("Copy Name", "이름 복사"),
    ("Copy as New Cell…", "새 셀로 복사…"),
    ("Start a new design from this one: every view, specs and notes", "이 셀에서 새 설계를 시작합니다: 모든 뷰, 스펙, 노트"),
    ("Rename…", "이름 바꾸기…"),
    ("Delete…", "삭제…"),
    ("Export Netlist…", "넷리스트 내보내기…"),
    ("Corner / Sweep Manager…", "코너 / 스윕 관리자…"),
    ("Monte Carlo…", "몬테카를로…"),
    ("Optimizer…", "최적화기…"),
    ("Derive New Design…", "새 설계 파생…"),
    ("Verification…", "검증 (Verification)…"),
    ("Silicon Correlation…", "실리콘 상관…"),
    ("Process Emulation…", "공정 에뮬레이션…"),
    ("Run DRC", "DRC 실행"),
    ("Extract Nets", "넷 추출"),
    ("Schematic from Layout", "레이아웃에서 회로도"),
    ("Pixel Circuit…", "픽셀 회로…"),
    ("Run LVS", "LVS 실행"),
    ("Run Sign-off Runset", "사인오프 런셋 실행"),
    ("Import Sign-off Results…", "사인오프 결과 가져오기…"),
    ("Generate Layout from Schematic", "회로도에서 레이아웃 생성"),
    ("Route Flylines", "플라이라인 배선"),
    ("Check Against Source", "소스와 비교"),
    ("Update Components and Nets", "소자와 넷 업데이트"),
    ("Update and Delete Extra Devices", "업데이트 후 남는 소자 삭제"),
    ("Check Schematic", "회로도 검사"),
    ("Place Cell Instance…", "셀 인스턴스 배치…"),
    ("Update Cell Instance Pins", "셀 인스턴스 핀 업데이트"),
    ("Edit Symbol Pins…", "심볼 핀 편집…"),
    ("Hierarchy Browser…", "계층 브라우저…"),
    ("Net Browser…", "넷 브라우저…"),
    ("Guard Ring…", "가드 링…"),
    ("Via Array…", "비아 배열…"),
    ("Matched Placement…", "정합 배치…"),
    ("PCell Designer…", "PCell 디자이너…"),
    ("Import Deck…", "데크 가져오기…"),
    ("Export Deck…", "데크 내보내기…"),
    ("Link Shared Deck…", "공유 데크 연결…"),
    ("Reload Shared Deck", "공유 데크 다시 읽기"),
    ("Unlink Shared Deck", "공유 데크 연결 해제"),
    ("Stream Layer Map…", "스트림 레이어 맵…"),
    ("Layer Display…", "레이어 표시…"),
    ("Dummy Fill…", "더미 필…"),
    ("OPC…", "OPC…"),
    ("Load SKILL (.il)…", "SKILL 불러오기 (.il)…"),
    ("Open a cell with a schematic and a layout first", "먼저 회로도와 레이아웃이 있는 셀을 여세요"),
    ("Open a project first", "먼저 프로젝트를 여세요"),
    ("Open a cell with a schematic first", "먼저 회로도가 있는 셀을 여세요"),
    ("Open a cell with a layout first", "먼저 레이아웃이 있는 셀을 여세요"),
    ("Open the demo project", "데모 프로젝트 열기"),
    ("Open a schematic", "회로도 열기"),
    ("Select a device", "소자 선택"),
    ("Run a simulation", "시뮬레이션 실행"),
    ("Measure a waveform", "파형 측정"),
    ("Open the layout", "레이아웃 열기"),
    ("Check the design rules", "설계 규칙 검사"),
    ("Compare layout and schematic", "레이아웃과 회로도 비교"),
    ("Start a new design from this one", "이 설계에서 새 설계 시작"),
    ("Find any command", "모든 명령 찾기"),
    (
        "IC Studio ships with example designs: inverters, logic gates, op-amps, SRAM. Open them to have a real circuit to explore.",
        "IC Studio에는 인버터, 로직 게이트, 연산증폭기, SRAM 같은 예제 설계가 들어 있습니다. 열어서 실제 회로를 살펴보세요.",
    ),
    (
        "Cells are listed in the Explorer on the left. Open the schematic of cmos_inverter: one PMOS and one NMOS transistor between vdd and ground.",
        "셀은 왼쪽 탐색기에 있습니다. cmos_inverter의 회로도를 여세요: vdd와 접지 사이에 PMOS 하나와 NMOS 하나가 있습니다.",
    ),
    (
        "Click a transistor on the canvas. Its parameters (W, L, model) appear in the panel on the right, where you can edit them. Scroll to pan, Ctrl+wheel to zoom, F to fit.",
        "캔버스에서 트랜지스터를 클릭하세요. 오른쪽 패널에 파라미터(W, L, 모델)가 나오고, 거기서 고칠 수 있습니다. 스크롤로 이동, Ctrl+휠로 확대/축소, F로 화면 맞춤.",
    ),
    (
        "The schematic is netlisted and simulated with the built-in SPICE engine, or with ngspice when it is installed. The analyses are set up in the cell's simulation settings.",
        "회로도가 넷리스트로 변환되어 내장 SPICE 엔진(또는 설치되어 있으면 ngspice)으로 시뮬레이션됩니다. 해석 종류는 셀의 시뮬레이션 설정에 있습니다.",
    ),
    (
        "Results open in the Simulation workspace. Click signal names to show or hide them, then open the Waveform Calculator to measure delays, rise times or averages.",
        "결과는 시뮬레이션 작업 공간에 열립니다. 신호 이름을 클릭해 보이거나 숨기고, 파형 계산기를 열어 지연, 상승 시간, 평균을 측정하세요.",
    ),
    (
        "The same cell has a layout: the shapes that go on silicon, drawn on the layers of the generic 180 nm technology. Layout coordinates are in nanometres.",
        "같은 셀에 레이아웃도 있습니다: 실리콘에 새겨질 도형을 범용 180 nm 공정의 레이어로 그린 것입니다. 레이아웃 좌표 단위는 나노미터입니다.",
    ),
    (
        "DRC finds shapes that are too narrow or too close together. Violations are listed in the Verify panel and marked on the canvas.",
        "DRC는 너무 좁거나 서로 너무 가까운 도형을 찾습니다. 위반 사항은 검증 패널에 나오고 캔버스에 표시됩니다.",
    ),
    (
        "LVS extracts the devices and nets from the layout and compares them with the schematic. Here it reports C1: the 20 fF load is a test load that exists only in the schematic, so the layout has no counterpart. Other differences would mean a wiring error.",
        "LVS는 레이아웃에서 소자와 넷을 추출해 회로도와 비교합니다. 여기서는 C1이 보고됩니다: 20 fF 부하는 회로도에만 있는 테스트용 부하라서 레이아웃에는 대응하는 것이 없습니다. 다른 차이는 배선 오류를 뜻합니다.",
    ),
    (
        "Most designs begin as a copy of an earlier one. With nothing selected, the right panel of a schematic shows the cell's targets (specs) and design notes. Click \"Derive a new design from this\", make a target stricter (for example tplh <= 40p), and IC Studio copies the cell, sizes the copy to meet it and lays it out. The original stays as it was.",
        "대부분의 설계는 이전 설계를 복사해서 시작합니다. 아무것도 선택하지 않으면 회로도 오른쪽 패널에 셀의 목표(스펙)와 설계 노트가 나옵니다. \"이 설계에서 새 설계 파생\"을 누르고 목표 하나를 더 엄격하게(예: tplh <= 40p) 바꾸면, IC Studio가 셀을 복사하고 그 목표에 맞게 크기를 정한 뒤 레이아웃까지 만듭니다. 원본은 그대로 남습니다.",
    ),
    (
        "Every action is in the command palette. Type a few letters, for example \"export gds\", and press Enter.",
        "모든 동작은 명령 팔레트에 있습니다. 예를 들어 \"export gds\"처럼 몇 글자 입력하고 Enter를 누르세요.",
    ),
    ("Home › Open demo project, or File › New Demo Project", "홈 › 데모 프로젝트 열기, 또는 파일 › 새 데모 프로젝트"),
    ("Explorer › work › cmos_inverter › Schematic", "탐색기 › work › cmos_inverter › Schematic"),
    ("Click a transistor", "트랜지스터를 클릭"),
    ("F5 or ⌘↩, or Simulation › Run", "F5 또는 ⌘↩, 또는 시뮬레이션 › 실행"),
    ("Simulation › Waveform Calculator…", "시뮬레이션 › 파형 계산기…"),
    ("⌘4, or the Layout icon on the left", "⌘4, 또는 왼쪽의 레이아웃 아이콘"),
    ("Verify › Run DRC", "검증 › DRC 실행"),
    ("Verify › Run LVS", "검증 › LVS 실행"),
    ("Schematic › right panel › Derive a new design from this…", "회로도 › 오른쪽 패널 › 이 설계에서 새 설계 파생…"),
    ("⌘K, or the search box at the top right", "⌘K, 또는 오른쪽 위 검색 상자"),
    ("Getting started", "시작하기"),
    ("Simulation, layout, PCells", "시뮬레이션, 레이아웃, PCell"),
    ("Analog", "아날로그"),
    ("Current mirror and OTAs", "전류 미러와 OTA"),
    ("Digital", "디지털"),
    ("Gates, ring osc, auto P&R", "게이트, 링 발진기, 자동 P&R"),
    ("Memory and pixels", "메모리와 픽셀"),
    ("SRAM, DRAM, flash, pixels", "SRAM, DRAM, 플래시, 픽셀"),
    ("Hierarchy", "계층"),
    ("8192 inverters, 2 levels", "인버터 8192개, 2단계"),
    ("Schematic", "회로도"),
    (
        "Place devices, wire nets, label pins. Live connectivity and netlisting.",
        "소자를 놓고 넷을 연결하고 핀에 이름을 붙입니다. 연결 상태와 넷리스트가 실시간으로 갱신됩니다.",
    ),
    ("OP, DC, AC and transient on the built-in engine or ngspice.", "내장 엔진이나 ngspice로 OP, DC, AC, 과도 해석."),
    (
        "Polygon editing on PDK layers, DRC and GDSII / OASIS import/export.",
        "PDK 레이어 위의 도형 편집, DRC, GDSII / OASIS 가져오기/내보내기.",
    ),
    ("Inductance", "인덕턴스"),
    ("Length (L)", "길이 (L)"),
    ("Width (W)", "폭 (W)"),
    ("Width", "폭"),
    ("Transresistance", "트랜스저항"),
    ("Resistance", "저항"),
    ("Capacitance", "커패시턴스"),
    ("DC value", "DC 값"),
    ("AC magnitude", "AC 크기"),
    ("Transient", "과도 (Transient)"),
    ("Model", "모델"),
    ("Multiplier (M)", "병렬 수 (M)"),
    ("Body net", "바디 넷"),
    ("Net name", "넷 이름"),
    ("Area factor", "면적 배수"),
    ("Gain", "이득"),
    ("Transconductance", "트랜스컨덕턴스"),
    ("Controlling source", "제어 소스"),
    (
        "Channel width. A wider transistor passes more current at the same voltages: faster and higher transconductance (gm), but larger, with more capacitance and power. Current scales about with W / L.",
        "채널 폭. 폭이 넓을수록 같은 전압에서 전류가 더 많이 흐릅니다: 더 빠르고 트랜스컨덕턴스(gm)가 크지만, 면적·커패시턴스·전력이 늘어납니다. 전류는 대략 W / L에 비례합니다.",
    ),
    (
        "Channel length (the process minimum is 180n). A longer transistor passes less current but behaves more like an ideal current source (higher output resistance, more gain) and matches better. Short L is fast; long L is for gain, mirrors and matching.",
        "채널 길이(공정 최소값은 180n). 길이가 길수록 전류는 줄지만 이상적인 전류원에 가까워지고(출력 저항과 이득 증가) 정합도 좋아집니다. 짧은 L은 빠르고, 긴 L은 이득·미러·정합용입니다.",
    ),
    (
        "Number of identical transistors in parallel. M = 2 acts like twice the width, and copies of the same size match better than one wide device.",
        "같은 트랜지스터를 병렬로 몇 개 놓는지. M = 2는 폭을 두 배로 한 것과 같고, 같은 크기를 여러 개 두면 넓은 소자 하나보다 정합이 좋습니다.",
    ),
    ("The device model of the PDK (nmos / pmos, or a PDK model name).", "PDK의 소자 모델 (nmos / pmos, 또는 PDK 모델 이름)."),
    (
        "The net the transistor body connects to: ground (0) for NMOS, the supply (vdd) for PMOS.",
        "트랜지스터 바디가 연결되는 넷: NMOS는 접지(0), PMOS는 전원(vdd).",
    ),
    (
        "Resistance in ohms, e.g. 10k. Larger R gives less current and a slower RC.",
        "저항값(옴), 예: 10k. R이 크면 전류가 줄고 RC가 느려집니다.",
    ),
    (
        "Capacitance in farads, e.g. 1p. Larger C stores more charge and slows the node down.",
        "커패시턴스(패럿), 예: 1p. C가 크면 전하를 더 저장하고 노드가 느려집니다.",
    ),
    ("Inductance in henries, e.g. 1n.", "인덕턴스(헨리), 예: 1n."),
    ("The constant value used by the operating point and DC sweep.", "동작점과 DC 스윕에서 쓰는 일정한 값."),
    (
        "Amplitude of the small signal for AC analysis. Set 1 on the input source to read gain directly.",
        "AC 해석용 소신호 크기. 입력 소스에 1을 넣으면 출력에서 이득을 바로 읽을 수 있습니다.",
    ),
    (
        "The waveform for transient analysis, e.g. PULSE(low high delay rise fall width period) or SIN(offset amp freq).",
        "과도 해석용 파형, 예: PULSE(low high delay rise fall width period) 또는 SIN(offset amp freq).",
    ),
    ("Number of identical devices in parallel.", "병렬로 놓는 같은 소자의 수."),
    (
        "Wider input transistors amplify more (higher gm): more gain and bandwidth, more input capacitance.",
        "입력 트랜지스터가 넓을수록 더 크게 증폭합니다(gm 증가): 이득과 대역폭이 늘고, 입력 커패시턴스도 늘어납니다.",
    ),
    (
        "Longer input transistors give more gain and better matching, but a slower amplifier.",
        "입력 트랜지스터가 길수록 이득과 정합이 좋아지지만 증폭기가 느려집니다.",
    ),
    (
        "Wider mirror transistors need less voltage headroom; the current ratio stays the same.",
        "미러 트랜지스터가 넓을수록 필요한 전압 여유가 줄어듭니다. 전류 비율은 그대로입니다.",
    ),
    (
        "Longer mirror transistors copy the current more exactly and, as a load, give more gain.",
        "미러 트랜지스터가 길수록 전류를 더 정확히 복사하고, 부하로 쓰면 이득이 커집니다.",
    ),
    (
        "The copied current is the reference times (W/L output) / (W/L input): a wider output copies more current.",
        "복사되는 전류 = 기준 전류 × (출력 W/L) / (입력 W/L): 출력 쪽이 넓을수록 더 많은 전류를 복사합니다.",
    ),
    (
        "The output's L sets the mirror ratio with its W; keep it equal to the input's L for a precise copy.",
        "출력의 L은 W와 함께 미러 비율을 정합니다. 정확한 복사를 위해 입력의 L과 같게 두세요.",
    ),
    (
        "With the same gate voltage, a wider current source passes more current: faster, more power.",
        "같은 게이트 전압에서 전류원이 넓을수록 전류가 더 흐릅니다: 더 빠르고, 전력이 늘어납니다.",
    ),
    (
        "A longer current source keeps its current steadier when its drain voltage moves.",
        "전류원이 길수록 드레인 전압이 변해도 전류가 더 일정하게 유지됩니다.",
    ),
    (
        "A wider transistor is stronger: faster edges and more current, more area and capacitance.",
        "넓은 트랜지스터는 더 강합니다: 엣지가 빠르고 전류가 많지만, 면적과 커패시턴스가 늘어납니다.",
    ),
    ("A longer transistor is weaker but has a higher output resistance.", "긴 트랜지스터는 약하지만 출력 저항이 높습니다."),
    ("input pair", "입력쌍"),
    ("current mirror", "전류 미러"),
    ("mirror output", "미러 출력"),
    ("current source", "전류원"),
    ("transistor", "트랜지스터"),
    ("minimum", "최소"),
    ("maximum", "최대"),
    ("Spec", "스펙"),
    ("Min", "최소"),
    ("Max", "최대"),
    ("Original", "원본"),
    ("Target", "목표"),
    ("Original sizes", "원래 크기"),
    ("new", "새 항목"),
    ("Next", "다음"),
    ("Skip", "건너뛰기"),
    ("No project open", "열린 프로젝트 없음"),
    ("Explorer", "탐색기"),
    ("Filter", "필터"),
    ("cell name", "셀 이름"),
    ("Search commands…", "명령 검색…"),
    ("Keep the supply current near the original (0.5x to 2x)", "공급 전류를 원본 근처로 유지 (0.5~2배)"),
    (
        "Without it the optimizer may meet a target by starving the circuit of current (slow) or flooding it (power). An idd target replaces it.",
        "이 옵션이 없으면 최적화기가 회로에 전류를 거의 주지 않거나(느려짐) 너무 많이 줘서(전력 증가) 목표를 맞출 수 있습니다. idd 목표를 직접 넣으면 그것이 대신합니다.",
    ),
    ("What else changed", "그 밖에 달라진 것"),
    ("resistor", "저항"),
    ("capacitor", "커패시터"),
    (
        "A resistor sets a current, a gain or, with its partner in a divider, a voltage ratio: an output set by R1 / R2 changes with that ratio.",
        "저항은 전류나 이득을 정하고, 분압기에서 짝 저항과 함께 전압 비율을 정합니다: R1 / R2로 정해지는 출력은 그 비율에 따라 바뀝니다.",
    ),
    (
        "A capacitor sets a pole: larger is slower and, for a compensation capacitor, more stable; smaller is faster and less stable. On chip, capacitors cost a lot of area.",
        "커패시터는 극점을 정합니다: 크면 느려지고 보상 커패시터라면 더 안정해지며, 작으면 빨라지고 덜 안정해집니다. 칩에서 커패시터는 면적을 많이 차지합니다.",
    ),
];

/// Sentences translated inside longer texts (assembled advice).
pub const SENTENCES: &[(&str, &str)] = &[
    (
        "Gain too low: first check that every amplifying transistor is in saturation (Simulation > Run, the operating point table). Then lengthen L of the amplifying transistors and their loads (higher output resistance), or lower the current per stage. Each change slows the circuit a little.",
        "이득이 너무 낮습니다: 먼저 증폭 트랜지스터가 모두 포화 영역에 있는지 확인하세요(시뮬레이션 > 실행, 동작점 표). 그다음 증폭 트랜지스터와 부하의 L을 늘리거나(출력 저항 증가), 단마다 전류를 줄이세요. 어느 쪽이든 회로가 조금 느려집니다.",
    ),
    (
        "Triode: vds is smaller than vgs - vth, so it acts like a resistor or a closed switch. Right for logic and switches. An amplifier, active load or current source needs saturation: give it more vds (headroom) or less overdrive vgs - vth (wider W for the same current).",
        "트라이오드: vds가 vgs - vth보다 작아서 저항이나 닫힌 스위치처럼 동작합니다. 로직과 스위치라면 정상입니다. 증폭기, 능동 부하, 전류원은 포화 영역이어야 합니다: vds 여유를 늘리거나 오버드라이브 vgs - vth를 줄이세요(같은 전류에서 W를 넓힘).",
    ),
    (
        "Too little phase margin: in feedback the circuit will ring or oscillate. Add or enlarge the compensation capacitor (a Miller capacitor between the stages of a two-stage amplifier), reduce the load capacitance, or lower the gain-bandwidth (less input-pair current).",
        "위상 여유가 너무 작습니다: 피드백을 걸면 회로가 울리거나 발진합니다. 보상 커패시터를 추가하거나 키우고(2단 증폭기라면 단 사이의 밀러 커패시터), 부하 커패시턴스를 줄이거나, 이득-대역폭을 낮추세요(입력쌍 전류 감소).",
    ),
    (
        "channel lengths changed, and the generic 180 nm models are SPICE level 1, whose output resistance does not depend on L: in silicon a longer L gives more gain and a shorter one less. Confirm with a PDK model.",
        "채널 길이가 바뀌었습니다. 범용 180 nm 모델은 SPICE level 1이라 출력 저항이 L에 따라 변하지 않습니다: 실제 실리콘에서는 L이 길면 이득이 커지고 짧으면 작아집니다. PDK 모델로 확인하세요.",
    ),
    (
        "Too slow: raise the bias current (e.g. the tail current source of a differential pair), widen the input transistors for more transconductance gm, or reduce the load capacitance. Speed costs power or gain.",
        "너무 느립니다: 바이어스 전류를 늘리거나(예: 차동쌍의 꼬리 전류원), 입력 트랜지스터를 넓혀 트랜스컨덕턴스 gm을 키우거나, 부하 커패시턴스를 줄이세요. 속도는 전력이나 이득과 맞바꿉니다.",
    ),
    (
        "Current too low: widen (W) or shorten (L) the transistor that sets it, or raise its gate drive. In a current mirror the output current is the reference times (W/L out) / (W/L reference).",
        "전류가 너무 작습니다: 전류를 정하는 트랜지스터를 넓히거나(W) 줄이거나(L), 게이트 구동을 높이세요. 전류 미러에서 출력 전류 = 기준 전류 × (출력 W/L) / (기준 W/L)입니다.",
    ),
    (
        "Current too high: narrow (W) or lengthen (L) the transistor that sets it, or lower its gate drive. In a current mirror, lower the (W/L out) / (W/L reference) ratio.",
        "전류가 너무 큽니다: 전류를 정하는 트랜지스터를 좁히거나(W) 늘리거나(L), 게이트 구동을 낮추세요. 전류 미러라면 (출력 W/L) / (기준 W/L) 비율을 낮추세요.",
    ),
    (
        "The waveform never reaches that level: the circuit may not switch at all (check the input source and the supply), or the simulation stops too early (longer tstop).",
        "파형이 그 레벨에 도달하지 않습니다: 회로가 아예 스위칭하지 않거나(입력 소스와 전원 확인), 시뮬레이션이 너무 일찍 끝납니다(tstop을 늘리세요).",
    ),
    (
        "Off: vgs is below the threshold vth, so no current flows. Right for an open switch; otherwise raise its gate voltage (or check the bias that drives it).",
        "꺼짐: vgs가 문턱 전압 vth보다 낮아 전류가 흐르지 않습니다. 열린 스위치라면 정상입니다. 아니라면 게이트 전압을 높이세요(또는 구동하는 바이어스를 확인).",
    ),
    (
        "Voltage out of range: it is set by a ratio (resistors, transistor sizes) or a bias; find the parameter with a sweep and move it the other way.",
        "전압이 범위를 벗어났습니다: 전압은 비율(저항, 트랜지스터 크기)이나 바이어스로 정해집니다. 스윕으로 해당 파라미터를 찾아 반대쪽으로 움직이세요.",
    ),
    (
        "The expression names a net or source the circuit does not have. Net names come from labels (v(out)); currents from voltage sources (i(v1)).",
        "식에 회로에 없는 넷이나 소스 이름이 있습니다. 넷 이름은 라벨에서(v(out)), 전류는 전압 소스에서(i(v1)) 옵니다.",
    ),
    (
        "Too slow: widen the transistors that drive the edge (PMOS for a rising output, NMOS for a falling one), or reduce the load capacitance.",
        "너무 느립니다: 해당 엣지를 구동하는 트랜지스터를 넓히거나(출력 상승은 PMOS, 하강은 NMOS), 부하 커패시턴스를 줄이세요.",
    ),
    (
        "Simulation > Optimizer can search the sizes against this spec, and the Corner / Sweep Manager shows which way one parameter moves it.",
        "시뮬레이션 > 최적화기로 이 스펙에 맞는 크기를 찾을 수 있고, 코너 / 스윕 관리자로 파라미터 하나가 스펙을 어느 쪽으로 움직이는지 볼 수 있습니다.",
    ),
    (
        "Weak inversion: vgs is just below vth. Little current and efficient, but slow; raise vgs (or shrink W) if it should be faster.",
        "약반전: vgs가 vth보다 조금 낮습니다. 전류가 작고 효율적이지만 느립니다. 더 빨라야 한다면 vgs를 높이세요(또는 W를 줄이세요).",
    ),
    (
        "Wider than allowed: enlarge the capacitor (or resistor) that sets the pole, or lower the bias current.",
        "허용보다 넓습니다: 극점을 정하는 커패시터(또는 저항)를 키우거나 바이어스 전류를 줄이세요.",
    ),
    (
        "Period too long (too slow): wider or shorter-L transistors, fewer stages or less load make it faster.",
        "주기가 너무 깁니다(너무 느림): 트랜지스터를 넓히거나 L을 줄이거나, 단 수나 부하를 줄이면 빨라집니다.",
    ),
    (
        "Period too short (too fast): add stages (keep an odd number in a ring), lengthen L or add load.",
        "주기가 너무 짧습니다(너무 빠름): 단을 추가하거나(링에서는 홀수 유지), L을 늘리거나 부하를 더하세요.",
    ),
    (
        "Settles too slowly: more bandwidth (bias current) and enough phase margin (about 60 degrees).",
        "안정되는 데 너무 오래 걸립니다: 대역폭(바이어스 전류)을 늘리고 위상 여유를 충분히(약 60도) 확보하세요.",
    ),
    (
        "Too much overshoot: the loop is close to unstable; improve the phase margin (compensation).",
        "오버슈트가 너무 큽니다: 루프가 불안정에 가깝습니다. 위상 여유를 늘리세요(보상).",
    ),
    (
        "The spec reads an analysis that is not enabled: turn it on in the cell's simulation setup.",
        "스펙이 켜져 있지 않은 해석을 읽습니다: 셀의 시뮬레이션 설정에서 그 해석을 켜세요.",
    ),
    (
        "Gain higher than allowed: shorter L, a resistive load or source degeneration lowers it.",
        "이득이 허용보다 높습니다: L을 줄이거나, 저항 부하나 소스 디제너레이션을 쓰면 낮아집니다.",
    ),
    (
        "More phase margin than needed: a smaller compensation capacitor would make it faster.",
        "위상 여유가 필요 이상입니다: 보상 커패시터를 줄이면 더 빨라집니다.",
    ),
    (
        "Faster than the minimum: narrower transistors or more load capacitance slow it down.",
        "최소값보다 빠릅니다: 트랜지스터를 좁히거나 부하 커패시턴스를 늘리면 느려집니다.",
    ),
    (
        "Gain margin out of range: change the compensation capacitor, as for phase margin.",
        "이득 여유가 범위를 벗어났습니다: 위상 여유와 마찬가지로 보상 커패시터를 조정하세요.",
    ),
    ("LVS: the transistors match the schematic", "LVS: 트랜지스터가 회로도와 일치"),
    ("LVS: the layout matches the schematic", "LVS: 레이아웃이 회로도와 일치"),
    ("DRC clean", "DRC 통과"),
];

/// Whole lines with `{}` parts (numbers, names) filled in.
pub const TEMPLATES: &[(&str, &str)] = &[
    (
        "Saturated, but only {} mV from triode (vds - (vgs - vth)): a small change in the voltages pushes it out. Give it more vds or less overdrive.",
        "포화 상태지만 트라이오드까지 {} mV밖에 남지 않았습니다(vds - (vgs - vth)): 전압이 조금만 변해도 벗어납니다. vds를 늘리거나 오버드라이브를 줄이세요.",
    ),
    ("placed {} device(s) like {}", "소자 {}개를 {}처럼 배치"),
    (
        "the reference placement did not fit the new sizes; generated {} device(s) from the schematic",
        "원본 배치에 새 크기가 맞지 않아 회로도에서 소자 {}개를 생성",
    ),
    ("generated {} device(s) from the schematic", "회로도에서 소자 {}개를 생성"),
    ("routed {} net(s) with {} connection(s), {} via(s)", "넷 {}개 배선, 연결 {}개, 비아 {}개"),
    ("DRC clean after {} automatic fix(es) of {} violation(s)", "자동 수정 {}건으로 위반 {}건을 고쳐 DRC 통과"),
    ("{} DRC violation(s) left (Ctrl+. in the layout offers fixes)", "DRC 위반 {}건 남음 (레이아웃에서 Ctrl+.로 수정안 보기)"),
    ("LVS: {} difference(s) (Verify > Run LVS lists them)", "LVS: 차이 {}건 (검증 > LVS 실행에서 목록 확인)"),
    ("routing failed: {}", "배선 실패: {}"),
    (
        "{} resistor(s) / capacitor(s) still to draw by hand (Create > PCell: poly resistor, MOM capacitor): the generator places transistors only",
        "저항 / 커패시터 {}개는 직접 그려야 합니다(만들기 > PCell: poly 저항, MOM 커패시터): 생성기는 트랜지스터만 배치합니다",
    ),
    (
        "{} left saturation (now {}): as an amplifier, load or current source it no longer works as intended",
        "{}이(가) 포화 영역을 벗어났습니다(현재 {}): 증폭기, 부하, 전류원이라면 더 이상 의도대로 동작하지 않습니다",
    ),
    ("{} is now only {} mV from leaving saturation", "{}이(가) 이제 포화 영역 경계까지 {} mV밖에 남지 않았습니다"),
    (
        "supply power {} -> {} W ({}x less): lower current usually also means a slower circuit",
        "전원 전력 {} -> {} W ({}배 감소): 전류가 줄면 보통 회로도 느려집니다",
    ),
    ("supply power {} -> {} W ({}x)", "전원 전력 {} -> {} W ({}배)"),
    (
        "{} ({}) have {}x less gate area: random mismatch (offset) about {}x larger",
        "{} ({})의 게이트 면적이 {}배 줄었습니다: 랜덤 부정합(오프셋)이 약 {}배 커집니다",
    ),
    (
        "{} ({}) got a shorter L ({} -> {}): lower output resistance and worse matching than the original",
        "{} ({})의 L이 짧아졌습니다({} -> {}): 원본보다 출력 저항이 낮고 정합이 나빠집니다",
    ),
    (
        "bias current of {}: {} -> {} A; speed and power follow it",
        "{}의 바이어스 전류: {} -> {} A. 속도와 전력이 이를 따라갑니다",
    ),
];

/// Korean design notes of the demo cells (see `ics_core::examples::design_notes`).
pub fn design_notes(cell: &str) -> Option<&'static str> {
    Some(match cell {
        "cmos_inverter" => {
            "무엇인가: 가장 기본적인 CMOS 논리 게이트입니다. in이 낮으면 PMOS가 out을 vdd로 끌어올리고, 높으면 NMOS가 끌어내립니다. C1(20 fF)은 출력이 구동하는 다음 게이트들을 대신합니다.\n\
             왜 이 크기인가: 속도를 위해 L은 공정 최소값(180n)입니다. 정공이 전자보다 느리기 때문에 PMOS를 두 배로 넓게(2u 대 1u) 해서 상승·하강 시간을 비슷하게 맞췄습니다.\n\
             스펙: 과도 해석에서 tphl(출력 하강) <= 50 ps, tplh(출력 상승) <= 70 ps.\n\
             새 설계로 파생하려면: 더 빠르거나 더 큰 부하를 구동하려면 두 W를 같은 비율로 키우고 2:1 비율은 유지하세요. 상승이 하강보다 느리면 PMOS를 넓히세요. C1을 실제 구동할 부하로 바꾸고 tphl / tplh를 다시 확인하세요."
        }
        "inv" => {
            "무엇인가: 핀 a(입력)와 y(출력)가 있는 재사용 블록 인버터로, buffer_tb에서 두 번 배치됩니다.\n\
             새 설계로 파생하려면: 복사해서 트랜지스터 크기를 바꾼 뒤, 이 셀을 배치한 회로도에서 도구 > 셀 인스턴스 핀 업데이트를 실행하세요."
        }
        "buffer_tb" => {
            "무엇인가: inv 셀 두 개를 직렬로 놓은 테스트벤치(tb)입니다. 버퍼는 약해진 신호를 반전 없이 복원합니다.\n\
             새 설계로 파생하려면: 큰 부하를 빠르게 구동하려면 두 번째 인버터를 첫 번째보다 3~4배 크게 하세요(테이퍼드 버퍼)."
        }
        "rc_lowpass" => {
            "무엇인가: 1차 저역 통과 필터입니다. fc = 1 / (2 pi R C)(1k, 1n이면 약 159 kHz)보다 높은 주파수는 디케이드당 20 dB씩 감쇠됩니다.\n\
             스펙: 대역폭 120~200 kHz, 상승 시간 <= 1.5 us. 공정 편차의 영향을 보기 위해 R과 C를 +-10 % 스윕합니다.\n\
             새 설계로 파생하려면: 먼저 C를 정하고(칩에서 커패시터는 면적이 크므로 작게), R = 1 / (2 pi fc C)로 구하세요. R이나 C를 절반으로 하면 fc는 두 배, 상승 시간은 절반이 됩니다."
        }
        "diode_clipper" => {
            "무엇인가: 직렬 저항과 접지로 가는 다이오드가 출력의 양(+) 반주기를 다이오드 전압 강하(약 0.7 V)로 제한합니다. 음(-) 반주기는 다이오드가 역방향이라 그대로 통과합니다.\n\
             새 설계로 파생하려면: R이 크면 클리핑할 때 전류가 줄어듭니다. 다이오드 두 개를 직렬로 하면 약 1.4 V에서 클리핑하고, 반대 방향 다이오드를 하나 더 넣으면 음 반주기도 클리핑합니다."
        }
        "bandgap" => {
            "무엇인가: 온도에 거의 변하지 않는 약 1.2 V 기준 전압입니다. Q2는 Q1보다 8배 크고, 두 베이스-이미터 전압의 차이는 온도에 따라 증가해 감소하는 Vbe를 상쇄합니다.\n\
             스펙: 0~100 C에서 vref 1.15~1.25 V.\n\
             새 설계로 파생하려면: R2 / R3 비율이 증가 성분을 얼마나 더할지 정합니다. vref가 온도에 따라 올라가면 R2 / R3를 낮추고, 내려가면 높이세요. 면적비(8)는 그대로 두어도 됩니다."
        }
        "current_mirror" => {
            "무엇인가: M1의 20 uA 기준 전류를 출력 M2로 복사합니다. M1은 다이오드 연결(게이트를 드레인에 연결)이라 기준 전류를 흘리는 게이트 전압이 저절로 정해지고, 같은 게이트 전압을 받는 M2도 같은 전류를 흘립니다.\n\
             왜 이 크기인가: L = 1u(최소보다 김)로 해서 출력 전류가 출력 전압에 덜 민감하고 M1과 M2의 정합이 좋습니다.\n\
             스펙: vout = 0.9 V에서 iout 19~21 uA.\n\
             새 설계로 파생하려면: iout / iref = (M2의 W/L) / (M1의 W/L)이므로, 2:1 미러는 M2를 두 배로 넓게(또는 m = 2) 하세요. 더 정확한 복사에는 긴 L, 전압 여유를 줄이려면 넓은 W. DC 스윕으로 vout을 얼마나 낮춰도 복사가 유지되는지 볼 수 있습니다."
        }
        "ota5t" => {
            "무엇인가: 가장 단순한 2입력 증폭기인 5-트랜지스터 OTA(operational transconductance amplifier)입니다. M1 / M2는 차동 입력쌍, M3 / M4는 PMOS 전류 미러 부하, M5는 vb로 정해지는 꼬리 전류원입니다.\n\
             왜 이 크기인가: 최소 길이보다 이득을 높이려고 L = 500n, 꼬리 전류가 일정하도록 M5는 더 긴 L = 1u입니다. 정공이 느리므로 PMOS 부하는 입력보다 두 배 넓습니다.\n\
             스펙: -40~125 C에서 이득 >= 30 dB, 단위 이득 주파수(ugf) >= 100 MHz, 위상 여유 >= 45도. 부하 커패시터가 없어서 ugf가 매우 높게 나옵니다: 현실적인 값을 보려면 out에 커패시터(예: 1p)를 다세요.\n\
             새 설계로 파생하려면: 이득을 높이려면 M1~M4의 L을 늘리세요. 대역폭을 늘리려면 꼬리 전류를 늘리거나(vb를 높이거나 M5를 넓힘) M1 / M2를 넓히세요. M1 = M2, M3 = M4(정합 쌍)를 유지하세요. 그다음 시뮬레이션 > 최적화기로 스펙에 맞게 크기를 정하고, 레이아웃 생성기 > By Example로 이 셀처럼 레이아웃을 만드세요."
        }
        "ota2s" => {
            "무엇인가: 2단 OTA입니다. 첫 단(M1~M5)은 5-트랜지스터 OTA이고, 전류원 M7을 단 두 번째 단 M6이 이득을 더합니다.\n\
             스펙: 이득 >= 70 dB.\n\
             주의: 보상 커패시터가 없어서 피드백을 걸면 이 증폭기는 발진합니다. 첫 파생 설계로 좋은 연습: 복사해서 o1과 out 사이에 밀러 커패시터 Cc(약 1p)를 추가하고, 위상 여유 스펙(phasemargin(v(out)) >= 60)을 더하세요. 완성된 버전이 ota_miller입니다.\n\
             새 설계로 파생하려면: 이득은 대략 두 단 이득의 곱이므로 L을 늘리면 커집니다. M6의 폭과 M7의 전류가 out의 속도를 정합니다."
        }
        "ota_miller" => {
            "무엇인가: 피드백에서 안정하도록 만든 2단 OTA입니다. 두 번째 단(M6, M7)이 이득을 더하고, 두 단 사이의 밀러 커패시터 Cc가 첫 단 출력을 느린 주극점(dominant pole)으로 만들어 루프에 충분한 위상 여유를 줍니다. Cc와 직렬인 Rz는 Cc가 만드는 영점을 상쇄합니다.\n\
             왜 이 크기인가: 출력 극점이 단위 이득 주파수보다 충분히 높도록 M6를 넓게(32u) 했습니다. Cc = 1p가 ugf = gm1 / (2 pi Cc)를 정합니다.\n\
             스펙: 1 pF 부하(CL, 테스트벤치 전용)에서 이득 >= 70 dB, ugf >= 25 MHz, 위상 여유 >= 60도.\n\
             새 설계로 파생하려면: 대역폭을 늘리려면 입력쌍의 gm(전류, 폭)을 키우거나 Cc를 줄인 뒤 위상 여유를 확인하세요. 출력 극점(gm6 / CL)이 ugf보다 2~3배 위에 있어야 하므로, 부하가 커지면 M6 / M7을 강하게 해야 합니다. Rz는 1 / gm6 근처나 조금 크게 할 때 여유가 가장 좋습니다."
        }
        "ota_cascode" => {
            "무엇인가: 텔레스코픽 캐스코드 OTA입니다. 캐스코드 트랜지스터(M3, M4와 M5, M6, vbn과 vbp로 바이어스)가 입력쌍과 미러 위에 쌓여 출력으로부터 이들을 차폐합니다. 그래서 출력 저항, 곧 단일 단의 이득이 높아지면서도 한 단의 속도와 낮은 전력을 유지합니다.\n\
             왜 이 크기인가: ota5t처럼 L = 500n, 꼬리(M9)는 L = 1u입니다. 바이어스 전압은 모든 소자를 포화 영역에 둡니다. 대가는 좁은 출력 범위입니다(쌓인 트랜지스터마다 vds가 필요).\n\
             스펙: 1 pF 부하에서 이득 >= 80 dB, ugf >= 20 MHz, 위상 여유 >= 60도.\n\
             새 설계로 파생하려면: 모든 소자가 포화 영역에 남도록 vbn, vbp를 유지하세요(파생 점검이 벗어난 소자를 경고합니다). 이득을 높이려면 L을 늘리고, 대역폭을 늘리려면 꼬리 전류를 늘리거나 입력쌍을 넓히세요."
        }
        "comparator" => {
            "무엇인가: 개방 루프 비교기입니다. inp가 inn(0.9 V)보다 높아지면 out이 높아집니다. 5-트랜지스터 OTA가 차이를 증폭하고, PMOS 단 M6 / M7이 이득을 더하고, 인버터 M8 / M9가 깨끗한 논리 엣지를 만듭니다.\n\
             왜 이 크기인가: 이득 단은 M7(게이트가 vb)의 전류로 제한되고, 이것이 지연을 주로 정합니다.\n\
             스펙: 50 mV 오버드라이브에서 입력 교차부터 출력 교차까지의 지연이 상승 <= 6 ns, 하강 <= 4 ns.\n\
             새 설계로 파생하려면: 더 빠르게 하려면 바이어스 전류를 늘리거나(vb, 또는 M5와 M7을 넓힘) M6를 넓히세요. 오버드라이브가 작을수록 느려집니다. 오프셋을 낮게 유지하려면 M1 = M2, M3 = M4를 지키세요."
        }
        "ldo" => {
            "무엇인가: 저전압 강하(LDO) 레귤레이터입니다. 1.8 V 전원에서 1 mA 부하에 일정한 1.2 V를 공급합니다. 오차 증폭기(M1~M5)가 분압된 출력 fb를 vref = 0.9 V와 비교하고, 둘이 같아질 때까지 큰 PMOS 패스 소자 MP의 게이트를 구동합니다: vout = vref (1 + R1 / R2).\n\
             왜 이 크기인가: MP를 아주 넓게(400u) 해서 부하 전류를 작은 전압 강하(드롭아웃)로 흘립니다. R1 / R2 = 1/3이 1.2 V를 정합니다. CL과 ILOAD는 부하(테스트벤치)입니다.\n\
             스펙: vout 1.17~1.23 V, 1 MHz에서 전원 제거비 >= 40 dB(전원 소스에 ac 1), 부하 포함 공급 전류 <= 1.1 mA.\n\
             새 설계로 파생하려면: 다른 출력 전압은 R1 / R2(또는 vref)를 바꾸세요. 부하 전류를 늘리려면 MP를 넓히세요. 전원 제거비를 높이려면 오차 증폭기의 이득(긴 L)이나 대역폭을 키우세요. 부하 변화가 크면 루프의 안정도(위상 여유)를 확인해야 합니다."
        }
        "nand2" => {
            "무엇인가: 2입력 NAND: a와 b가 모두 높을 때만 출력이 낮아집니다. PMOS 두 개는 병렬로 끌어올리고, NMOS 두 개는 직렬로 끌어내립니다.\n\
             왜 이 크기인가: 직렬 NMOS가 각각 1u라서 인버터보다 끌어내리는 속도가 느립니다. 상승·하강을 맞추려면 NMOS를 2u로 넓히세요.\n\
             스펙: tpd(a 상승 → out 하강) <= 40 ps.\n\
             새 설계로 파생하려면: 3입력 NAND는 PMOS 하나를 병렬로, NMOS 하나를 직렬로 더합니다. 직렬 NMOS는 입력 수만큼 넓히세요."
        }
        "nor2" => {
            "무엇인가: 2입력 NOR: a와 b가 모두 낮을 때만 출력이 높아집니다. PMOS 두 개는 직렬로 끌어올리고, NMOS 두 개는 병렬로 끌어내립니다.\n\
             왜 이 크기인가: 직렬 PMOS는 느려서 실제 라이브러리에서는 PMOS를 2~4배 넓게 만듭니다. 이 때문에 보통 NOR보다 NAND를 선호합니다.\n\
             스펙: tpd(a 상승 → out 하강) <= 20 ps."
        }
        "ring_osc5" => {
            "무엇인가: 인버터 다섯 개를 고리로 연결한 링 발진기입니다. 반전 횟수가 홀수라 안정되지 않고 발진하며, 주기는 단 지연 합의 두 배입니다. 공정 속도를 재는 데 쓰입니다.\n\
             스펙: 주기 120~170 ps.\n\
             새 설계로 파생하려면: 단 수를 늘리거나(항상 홀수) L을 늘리면 느려집니다. 부하 없이 트랜지스터만 넓히면 속도는 거의 같습니다."
        }
        "full_adder" => {
            "무엇인가: 28-트랜지스터 미러 전가산기(s = a xor b xor ci, co = a, b, ci의 다수결)로, 넷리스트에서 자동으로 배치·배선했습니다.\n\
             새 설계로 파생하려면: 넷리스트의 크기를 고친 뒤 검증 > 회로도에서 레이아웃 생성과 플라이라인 배선으로 레이아웃을 다시 만드세요."
        }
        "sram6t" => {
            "무엇인가: 6-트랜지스터 정적 메모리 비트입니다. 교차 결합된 두 인버터(P1/N1, P2/N2)가 비트를 저장하고, 워드 라인이 높을 때 접근 트랜지스터 A1 / A2가 비트 라인과 연결합니다.\n\
             왜 이 크기인가: 읽기 중에 비트가 뒤집히지 않도록 풀다운 NMOS가 접근 NMOS보다 강해야 하고(셀 비율), 쓰기가 되도록 접근 NMOS가 PMOS보다 강해야 합니다(풀업 비율)."
        }
        "dram1t1c" => {
            "무엇인가: 트랜지스터 하나와 커패시터 하나로 된 동적 메모리 비트입니다. 커패시터의 전하가 비트이고, 새어 나가므로 주기적으로 리프레시해야 합니다.\n\
             새 설계로 파생하려면: 저장 커패시터가 클수록 비트 라인의 읽기 신호가 커지고 리프레시 간격이 길어지지만, 면적이 늘어납니다."
        }
        "nand_string" => {
            "무엇인가: NAND 플래시 스트링: 비트 라인 콘택과 소스 라인 콘택 사이에 메모리 트랜지스터가 직렬로 있고, 양 끝에 선택 트랜지스터가 있습니다."
        }
        "aps4t" | "aps2s" => {
            "무엇인가: 이미지 센서 픽셀입니다. 포토다이오드가 빛으로 전하를 모으고, 전송 게이트(TG)가 그 전하를 플로팅 디퓨전(FD)으로 옮기며, 소스 팔로워(SF)가 행 선택(SEL)을 거쳐 FD를 읽고, RST가 FD를 리셋합니다. aps2s는 포토다이오드 두 개가 읽기 회로 하나를 공유합니다.\n\
             새 설계로 파생하려면: 검증 > 픽셀 회로에서 변환 이득, 풀 웰, 래그, 노이즈를 볼 수 있습니다. FD 커패시턴스가 작을수록 변환 이득이 커지고(노이즈 감소) 전압 범위는 줄어듭니다."
        }
        _ => return None,
    })
}
