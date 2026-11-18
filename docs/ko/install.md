<!-- seo title="IC Studio 설치 (Windows, macOS, Linux)" description="Windows, macOS, Linux에서 명령 한 줄로 IC Studio를 설치하거나 Rust로 소스에서 빌드하는 방법. SPICE 시뮬레이션용 ngspice 설정 포함." -->

# 설치

릴리스 빌드를 명령 하나로 설치하거나, Rust 툴체인으로 IC Studio를 소스에서 빌드할 수 있습니다.

## 릴리스 설치

Windows (PowerShell):

```powershell
irm https://github.com/geniuskey/ic-studio/releases/latest/download/install.ps1 | iex
```

macOS (Apple silicon)와 Linux (x86_64):

```sh
curl -fsSL https://github.com/geniuskey/ic-studio/releases/latest/download/install.sh | sh
```

설치 프로그램은 최신 릴리스를 내려받아 두 프로그램을 한 폴더에 설치합니다. 데스크톱 앱
`ic-studio`와 명령줄 도구 `ics`이며, `ics`는 AI 에이전트용 MCP 서버(`ics mcp`)이기도 합니다.

| | Windows | macOS / Linux |
|---|---|---|
| 폴더 | `%LOCALAPPDATA%\Programs\IC Studio` | `~/.local/share/ic-studio` |
| PATH 등록 | 폴더를 사용자 `PATH`에 추가 | `~/.local/bin`에 링크 생성 |
| 실행 아이콘 | 시작 메뉴 바로 가기 **IC Studio** | `~/.local/share/applications/ic-studio.desktop` (Linux) |

설치 프로그램은 다음 작업도 수행합니다.

- `claude`가 `PATH`에 있으면 Claude Code에 MCP 서버를 등록합니다
  (`claude mcp add --scope user ic-studio -- <folder>/ics mcp`). 없으면 이 명령과 다른 클라이언트용
  JSON 설정을 출력합니다([MCP 서버](mcp.md) 참고).
- [예제 프로젝트](demos.md#예제-프로젝트)를 주제별 폴더로 `Documents\IC Studio\Examples`(Windows)
  또는 `~/Documents/IC Studio/Examples`(macOS / Linux)에 씁니다. 이미 있는 예제는 그대로 두므로
  직접 수정한 내용은 업데이트 후에도 유지됩니다.
- macOS에서는 다운로드 격리(quarantine) 속성을 제거합니다.
- macOS / Linux에서 `~/.local/bin`이 `PATH`에 없으면 셸 프로필에 추가할 줄을 출력합니다.
  Windows에서는 새 터미널을 열어야 `PATH` 변경이 반영됩니다.

업데이트하려면 설치 프로그램을 다시 실행합니다. 앱이나 에이전트가 실행 중이어도 동작합니다(이후
다시 시작하세요). 앱 자체 업데이트 기능(**Help > Check for Updates...**)도 앱과 함께 `ics`를
교체하며, 새 버전은 처음 시작할 때 `Documents/IC Studio/Examples`에 없는 예제 프로젝트를 추가합니다.

**Settings**에서 **Check for updates**가 켜져 있으면(기본값) 앱은 시작할 때 새 릴리스를 확인하고
**Update now**, **Release notes**, **Later** 버튼이 있는 **Update available** 알림을 표시합니다.
업데이트 기능은 이전 버전의 `ics mcp` 서버가 아직 실행 중이어도 프로그램을 교체할 수 있으며,
설치가 도중에 실패하면 이전 버전을 복원합니다.

옵션은 환경 변수로 지정합니다.

| 변수 | 효과 |
|---|---|
| `ICS_VERSION` | 이 릴리스 태그를 설치, 예: `v0.2.0` (기본값: 최신) |
| `ICS_INSTALL_DIR` | 이 폴더에 설치 |
| `ICS_EXAMPLES_DIR` | 예제 프로젝트를 이 폴더에 씀 |
| `ICS_EXAMPLES=0` | 예제 프로젝트를 쓰지 않음 |
| `ICS_UNINSTALL=1` | 폴더, `PATH` 항목 또는 링크, 실행 아이콘, MCP 등록을 제거 (프로젝트와 예제 프로젝트는 유지) |

```powershell
$env:ICS_VERSION="v0.2.0"; irm https://github.com/geniuskey/ic-studio/releases/latest/download/install.ps1 | iex
```

```sh
curl -fsSL https://github.com/geniuskey/ic-studio/releases/latest/download/install.sh | ICS_UNINSTALL=1 sh
```

릴리스 빌드는 Windows x86_64, Linux x86_64, Apple silicon macOS용으로 제공됩니다. Intel Mac에서는
소스에서 빌드하세요. Linux 빌드는 Rocky Linux 8에서 만들어 glibc 2.28 이상이면 동작하므로, RHEL /
Rocky / Alma Linux 8 이상과 최신 Ubuntu, Debian, Fedora에서 실행됩니다.

**서명된 릴리스.** 모든 릴리스는 `SHA256SUMS`에 각 파일의 SHA-256을 담고, 설치 스크립트와 앱 내
업데이트가 이를 확인합니다. 저장소에 서명용 secret이 있으면 릴리스 워크플로가 바이너리에 서명도
합니다. macOS는 `APPLE_CERT_P12`(base64로 인코딩한 Developer ID Application 인증서),
`APPLE_CERT_PASSWORD`, `APPLE_SIGN_IDENTITY`와 notarization용 `APPLE_ID`, `APPLE_TEAM_ID`,
`APPLE_APP_PASSWORD`, Windows는 `WINDOWS_CERT_PFX`(base64 코드 서명 인증서)와
`WINDOWS_CERT_PASSWORD`를 씁니다. 이 secret이 없으면 서명되지 않은 바이너리가 나가며, macOS는 첫 실행
때 확인을 요구하고(오른쪽 클릭 > **열기**) Windows SmartScreen은 **추가 정보 > 실행**을 보여 줍니다.

**수동 다운로드.** 각 [릴리스](https://github.com/geniuskey/ic-studio/releases)에는 두 프로그램이
들어 있는 압축 파일 `ic-studio-<platform>.zip`(Windows) 또는 `ic-studio-<platform>.tar.gz`(macOS,
Linux)가 있으며, `<platform>`은 `windows-x86_64`, `linux-x86_64`, `macos-aarch64` 중 하나입니다.
원하는 폴더에 풀고 두 프로그램을 같은 폴더에 두세요. 업데이트 기능과 **Add to Claude Code**는
`ic-studio` 옆에서 `ics`를 찾습니다. 터미널에서 `ics`를 쓰려면 그 폴더를 `PATH`에 추가합니다.

## 소스에서 빌드

### 요구 사항

- [rustup.rs](https://rustup.rs)의 **Rust 1.95 이상**.
- **Windows**: MSVC 툴체인(`x86_64-pc-windows-msvc`). "Desktop development with C++" 워크로드가
  포함된 Visual Studio Build Tools가 필요합니다.
- **Linux**: 윈도 시스템 라이브러리
  `libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev`.
- **macOS**: Xcode 명령줄 도구.
- `wgpu`가 지원하는 GPU 드라이버(Windows는 DX12, Linux는 Vulkan, macOS는 Metal).

### 빌드와 실행

```sh
git clone https://github.com/geniuskey/ic-studio
cd ic-studio
cargo run --release                     # 데스크톱 앱
cargo run --release -- --demo           # 내장 데모 설계와 함께
cargo run --release -- path/to/project  # 프로젝트 폴더 열기
```

두 프로그램(`ic-studio`, `ics`)을 `target/release/`에 빌드하려면:

```sh
cargo build --release --workspace
```

## 실행 옵션

```text
ic-studio [PROJECT_DIR] [--demo] [--cell NAME] [--workspace home|schematic|simulation|layout] [--run]
```

| 옵션 | 효과 |
|---|---|
| `PROJECT_DIR` | 이 프로젝트 폴더를 엶 |
| `--demo` | 데모 프로젝트를 엶 (메모리에만 있으며 저장하기 전까지 디스크에 저장되지 않음) |
| `--cell NAME`, `-c NAME` | 이 이름의 셀을 엶 |
| `--workspace W`, `-w W` | 이 워크스페이스로 시작 |
| `--run` | 시작할 때 셀의 시뮬레이션을 실행 |

예: `cargo run --release -- --demo --cell inv_tiles --workspace layout`.

인수 없이 실행하면 Settings에서 **Reopen last project**가 체크되어 있을 때(기본값) 가장 최근
프로젝트를 다시 열고, 그렇지 않으면 시작 페이지를 표시합니다.

## ngspice (선택 사항)

IC Studio에는 자체 내장 시뮬레이터가 있으므로 ngspice가 꼭 필요하지는 않습니다. ngspice를
백엔드로 사용하려면 설치하세요.

- Windows: 공식 빌드, `C:\Spice64\bin\`에 설치됨
- macOS: `brew install ngspice`
- Linux: `apt install ngspice` (또는 배포판의 패키지)

IC Studio는 `/opt/homebrew/bin/ngspice`, `/usr/local/bin/ngspice`, `/usr/bin/ngspice`,
`C:\Spice64\bin\ngspice.exe`를 차례로 찾은 다음 `PATH`에서 `ngspice`를 찾습니다. 다른 위치를
쓰려면 **File > Settings...**(`Ctrl+,`)를 열고 **ngspice** 필드에 경로를 입력한 뒤 **Detect**를
누릅니다. 상태 표시줄과 콘솔에 ngspice를 찾았는지 표시됩니다.

Windows에서 `ngspice.exe`는 GUI 빌드입니다. 감지나 배치(batch) 시뮬레이션이 실패하면 같은 폴더의
콘솔 바이너리 `ngspice_con.exe`를 지정하세요.

## 설정

**File > Settings...**(`Ctrl+,`, **Tools** 메뉴나 액티비티 바 아래쪽의 톱니바퀴로도 열 수 있음):

| 설정 | 의미 |
|---|---|
| Theme | Dark 또는 Light (**View > Light Theme / Dark Theme**으로도 전환) |
| UI scale | 0.75 ~ 1.75 |
| ngspice | ngspice 실행 파일 경로, **Detect** 버튼 포함 |
| Interaction | 마우스 프로필: **Virtuoso**(기본값) 또는 **Conventional** ([단축키](shortcuts.md#캔버스-회로도와-레이아웃) 참고) |
| Startup | **Reopen last project**, 시작 시 **Check for updates** |
| AI | **AI Models...**: Design Copilot의 공급자, 모델, API 키 |
| AI agents (MCP) | **Add to Claude Code**는 `ics mcp`를 Claude Code에 등록하고, **Copy MCP config**는 다른 클라이언트용 JSON을 복사함 ([MCP 서버](mcp.md) 참고) |

설정과 최근 프로젝트 목록은 앱이 세션 간에 저장합니다.

## 실제 PDK를 쓰는 예제 프로젝트

스크립트로 오픈 소스 설계를 `examples/`(git 무시 대상)에 바로 열 수 있는 프로젝트로 내려받습니다.

```sh
./scripts/fetch-examples.sh             # macOS / Linux (curl, gunzip)
./scripts/fetch-examples.ps1            # Windows PowerShell
#   --caravel / -Caravel  전체 caravel 칩도 포함 (다운로드 52 MB, 압축 해제 후 278 MB)
#   --force   / -Force    내려받은 파일로 프로젝트를 다시 생성
```

- `examples/sky130`: efabless caravel 라이브러리 `caravel_clocking`, `housekeeping`, `chip_io`
  (그리고 `caravel`)를 포함한 SKY130 레이어 세트.
- `examples/gf180`: `gf180mcu_fd_sc_mcu7t5v0` 표준 셀과 `gf180mcu_fd_ip_sram` 512 x 8 SRAM 매크로를
  포함한 GF180MCU 레이어 세트.

**File > Open Project...** 또는 `ic-studio examples/sky130`으로 엽니다.
