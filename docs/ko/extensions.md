<!-- seo title="확장" description="서드파티 확장으로 IC Studio 기능 추가: 도구, 메뉴 명령, SKILL 스크립트, PDK를 어떤 언어로든 작성하고 git이나 커뮤니티 인덱스에서 설치." -->

# 확장

확장(extension)은 IC Studio 자체를 고치지 않고 기능을 더합니다. 누구나 확장을 만들어 자기 git 저장소에
공개하고 커뮤니티 인덱스에 올릴 수 있습니다. 확장이 더할 수 있는 것은 다음과 같습니다.

- **도구**: 확장의 프로그램이 제공하는 함수로, 어떤 언어로든 작성할 수 있습니다(Python이 가장 쉽습니다).
  기본 도구가 쓰이는 모든 곳에서 쓰입니다. **Extensions** 메뉴, 명령 팔레트, Design Copilot
  (**Tools > Design Copilot**), [MCP 서버](mcp.md)를 통한 AI 에이전트, [명령줄](cli.md),
  [Python API](python.md). 도구가 실행되는 동안 프로그램은 열린 프로젝트에 대해 모든 기본 도구를
  호출할 수 있습니다. 넷리스트 읽기, 시뮬레이션, DRC와 LVS, 레이아웃 그리기 등입니다.
- **메뉴 명령**: **Extensions** 아래 항목으로, 도구(확장의 것이든 기본 도구든)나 Cadence SKILL
  스크립트를 활성 셀에 실행합니다.
- **PDK**: **Extensions > Load PDK**에 나오는 [PDK 파일](technology.md).

확장은 `extension.toml`이 있는 폴더입니다. SKILL 스크립트나 PDK 파일만 있는 확장은 프로그램이 필요
없습니다.

> 프로그램이 있는 확장은 내려받은 다른 프로그램처럼 사용자 권한으로 실행됩니다. 믿을 수 있는 확장만
> 설치하고, 확실하지 않으면 코드를 읽어 보세요.

## 확장 사용하기

### 앱에서

**Extensions > Manage Extensions…**는 설치된 확장과 그 도구, 명령, PDK, 실행하는 프로그램, 설치 출처를
보여 줍니다. 여기서 다음을 할 수 있습니다.

- 확장 끄고 켜기(**Enabled**)
- **Install from Folder…**, 또는 저장소 URL(필요하면 하위 폴더와 태그나 브랜치)로 **Install from Git**
- **Community index**에서 **Load Index**로 인덱스를 불러와 검색하고 **Install**
- 설치한 곳에서 다시 받는 **Update**, **Remove**, 폴더와 웹사이트 열기, 실행 중인 프로그램 **Stop**
  (필요할 때 다시 시작됩니다)

명령이 있는 확장은 **Extensions** 아래에 하위 메뉴가 생기고, 명령은 명령 팔레트(⌘K / Ctrl+K)에도
나옵니다. 확장이 한 편집은 다른 편집처럼 되돌릴 수 있고, 확장의 로그는 콘솔에 나옵니다.

Design Copilot은 켜진 확장의 도구를 모두 압니다. 예를 들어 design-report 확장을 설치했다면 "nand2의
설계 보고서를 써 줘"라고 요청할 수 있습니다.

### 명령줄에서

```sh
ics ext search                      # 커뮤니티 인덱스
ics ext install design-report       # 인덱스 id로
ics ext install https://github.com/you/ics-thing --rev v1.0   # 아무 git 저장소
ics ext install ./my-extension      # 로컬 폴더(복사됨)
ics ext list                        # 설치된 확장: 도구, 명령, PDK, 상태
ics ext disable design-report       # enable / disable / update / remove <id>
ics ext call myproj design-report__report '{"cell": "nand2"}'
ics ext run myproj metal-frame/frame --cell nand2
```

`ics ext call`은 확장의 것이든 기본 도구든 저장된 프로젝트에 실행하고, 도구가 프로젝트를 바꿨으면
저장합니다(`--no-save`로 저장하지 않음). `ics ext run`은 메뉴 명령 `<확장 id>/<명령 id>`를 실행하며,
`${cell}`은 `--cell`로 정합니다.

git에서 설치하려면 `git`이 필요하고, 인덱스는 `curl`로 내려받습니다(둘 다 macOS, Linux, Windows 10
이상에 있습니다. Windows의 git은 Git for Windows).

### 도구 이름

확장의 도구 이름은 어디서나 `<확장 id>__<도구>`(밑줄 두 개)입니다. 예: `design-report__report`.
`ics mcp`의 도구 목록, Copilot, `ics ext call`, Python의 `Project.call`에서 모두 같습니다.

### 확장 폴더

설치된 확장은 사용자별 확장 폴더의 하위 폴더입니다.

| 시스템 | 폴더 |
|---|---|
| Windows | `%APPDATA%\ic-studio\extensions` |
| macOS | `~/Library/Application Support/ic-studio/extensions` |
| Linux | `$XDG_DATA_HOME/ic-studio/extensions` (`~/.local/share/ic-studio/extensions`) |

`ICS_EXTENSIONS_DIR`로 다른 폴더(예: 팀이 공유하는 폴더)를 쓸 수 있고, `ICS_NO_EXTENSIONS=1`이면
`ics mcp`가 확장 없이 시작합니다. 확장 폴더를 직접 복사해 넣었다면 Extensions 창의 **Reload**나 명령
팔레트의 **Reload extensions**를 실행합니다.

## 확장 만들기

템플릿에서 시작합니다.

```sh
ics ext new my-tool                      # 도구 하나와 메뉴 명령이 있는 Python 프로그램
ics ext new my-script --template skill   # 프로그램 없이 SKILL 스크립트 하나를 메뉴 명령으로
ics ext check my-tool                    # extension.toml 검사
ics ext install my-tool                  # 확장 폴더에 복사
```

개발 중에는 고칠 때마다 `ics ext install my-tool --force`로 다시 설치하거나, `ICS_EXTENSIONS_DIR`을
작업 폴더를 담은 폴더로 지정해 복사 없이 씁니다.

저장소의 [`extensions/`](https://github.com/geniuskey/ic-studio/tree/main/extensions) 폴더에 완전한
예제가 두 개 있습니다. `design-report`(Python: IC Studio를 통해 소자를 세고 DRC와 LVS를 실행해
Markdown으로 씀)와 `metal-frame`(SKILL 스크립트 하나)입니다.

### extension.toml

```toml
[extension]
id = "design-report"              # 소문자, 숫자, '-'; 인덱스에서 유일해야 함
name = "Design report"
version = "0.1.0"                 # x.y.z
description = "Writes a Markdown report of a cell: devices, DRC and LVS."
authors = ["Jane Doe"]
license = "MIT"
repository = "https://github.com/jane/ics-design-report"
ics = ">=0.2.9"                   # 동작하는 IC Studio 버전

[run]                             # 도구를 제공하는 프로그램
command = ["python3", "main.py"]  # 확장 폴더 기준 상대 경로, 아니면 PATH에서 찾음
windows = ["python", "main.py"]   # 선택: Windows에서는 command 대신 사용
env = { MY_SETTING = "1" }        # 선택: 추가 환경 변수

[[tools]]
name = "report"                   # design-report__report가 됨
description = "Check a cell and write reports/<cell>.md."
read_only = false                 # true면 Copilot이 plan 모드에서도 사용 가능
input_schema = { type = "object", properties = { cell = { type = "string" } }, required = ["cell"] }

[[commands]]
id = "report"
title = "Design report of the active cell"
tool = "report"                   # 확장의 도구, 또는 run_drc 같은 기본 도구
arguments = { cell = "${cell}" }

[[commands]]
id = "frame"
title = "Draw a metal frame"
skill = "scripts/frame.il"        # 활성 셀에 실행하는 SKILL 스크립트

[[pdks]]
name = "My 130 nm PDK"
file = "pdk/my130.toml"
```

| 키 | 의미 |
|---|---|
| `extension.ics` | 쉼표로 구분한 버전 비교(`>=`, `>`, `<=`, `<`, `=`). 버전만 쓰면 `>=`. 다른 버전용 확장은 목록에는 나오지만 쓰이지 않습니다. |
| `run.command` | 프로그램과 인자. 인자와 `env` 값의 `${extension}`은 확장 폴더입니다. |
| `tools[].description` | 도구가 하는 일. 사람과 도구를 고르는 AI 에이전트 모두를 위한 것이니 무엇이 필요하고 무엇을 돌려주는지 적습니다. |
| `tools[].input_schema` | 인자의 JSON Schema(TOML로 작성). 없으면 인자가 없습니다. |
| `commands[].arguments` | 고정 인자. 문자열 안의 `${cell}`은 활성 셀(`lib/cell`), `${project}`는 프로젝트 폴더, `${extension}`은 확장 폴더입니다. `${cell}`이 필요한 명령은 열린 셀이 있어야 합니다. |
| `commands[].skill` | 확장 안의 SKILL 파일. [`run_skill`](mcp.md) 도구처럼 활성 셀에 실행됩니다. 파일은 프로젝트 폴더 아래만, 환경 변수 없음, 최대 120초, 되돌리기 가능. |

### Python 도구

IC Studio는 프로그램을 시작할 때 도우미 모듈 `ics_extension`(표준 라이브러리만 사용)을 `PYTHONPATH`에
넣으므로 따로 설치할 것이 없습니다.

```python
from ics_extension import Extension, ToolError

ext = Extension()

@ext.tool("report")
def report(cell, write=True):
    deck = ext.call("get_netlist", cell=cell)["deck"]   # 아무 기본 도구
    drc = ext.call("run_drc", cell=cell)
    ext.log(f"{cell}: {drc['violations']} DRC violations")  # IC Studio 콘솔로
    return {"cell": cell, "violations": drc["violations"],
            "message": f"{cell}: DRC {drc['violations']}"}   # 콘솔에 표시

if __name__ == "__main__":
    ext.run()
```

- 도구는 JSON 값(dict, list, 문자열, 숫자)을 돌려줍니다. 오류는 예외로 알리며, 프로그램은 다음 호출을
  계속 처리합니다.
- `ext.call(tool, **arguments)`는 창에 열린 프로젝트(또는 `ics ext call` / `ics mcp`가 연 프로젝트)에
  기본 도구를 실행하고 결과를 돌려줍니다. 도구가 실패하면 `ToolError`가 납니다. 도구와 인자는
  [MCP 서버](mcp.md)의 것과 같습니다. 다른 확장의 도구는 호출할 수 없습니다.
- stdout에 출력하지 마세요. stdout은 프로토콜 통로입니다. `print`는 stderr로 돌려지며, 프로그램이
  실패하면 IC Studio가 그 내용을 보여 줍니다.
- 결과에 `"message"` 문자열이 있으면 메뉴에서 실행할 때 콘솔에 나옵니다.
- 프로그램은 도구가 처음 호출될 때 시작해 계속 실행되므로 호출 사이에 상태(불러온 모델, 캐시)를 유지할
  수 있습니다.

`ics_extension.py`는 MIT 라이선스이므로 확장에 복사해 넣어도 됩니다.

### 다른 언어

프로그램은 stdin과 stdout으로 줄 단위 [JSON-RPC 2.0](https://www.jsonrpc.org/specification)을
주고받고, stderr는 로그입니다. 메시지는 [Model Context Protocol](https://modelcontextprotocol.io)의
부분집합이라, 도구를 제공하는 기존 MCP 서버는 그대로 확장으로 쓸 수 있습니다(IC Studio를 다시 호출하지는
못합니다).

IC Studio → 프로그램:

| 메시지 | 의미 |
|---|---|
| `initialize`(요청) | 첫 메시지. `params`: `protocolVersion`(MCP), `icsProtocol`(이 프로토콜, `1`), `clientInfo` / `hostInfo` `{name, version}`, `extension` `{id, dir}`. 아무 결과로 답합니다. 예: `{"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}}` |
| `notifications/initialized` | `initialize` 응답 뒤에 옵니다. |
| `tools/call`(요청) | `{name, arguments}`: 도구 실행(`name`은 id 없이 `extension.toml`의 이름). `{content: [{type: "text", text}], isError}`로 답합니다. 다른 형태의 결과는 도구의 JSON 값으로 봅니다. |
| `shutdown`(알림) | 바로 종료합니다. 이어서 stdin이 닫힙니다. |

프로그램 → IC Studio(`tools/call`을 처리하는 동안):

| 메시지 | 의미 |
|---|---|
| `ics/call`(요청) | `{name, arguments}`: 열린 프로젝트에 기본 도구 실행. 응답은 `tools/call` 형태이고 text는 도구의 JSON 결과입니다. |
| `ics/log`(알림) | `{level, message}`: 콘솔 한 줄(`info`, `warning`, `error`). |

환경 변수: `ICS_EXTENSION_ID`, `ICS_EXTENSION_DIR`, `ICS_VERSION`. 작업 폴더는 확장 폴더입니다.
호출이 30분을 넘으면 프로그램을 멈춥니다.

### 공개하기

1. 확장을 README와 라이선스가 있는 공개 git 저장소(루트나 하위 폴더)에 올립니다. 사용자가 버전을 고정할
   수 있게 릴리스에 태그(`v0.1.0`)를 붙입니다.
2. `ics ext check`로 검사하고, 저장소 URL로 한 번 설치해 봅니다.
3. [`extensions/index.json`](https://github.com/geniuskey/ic-studio/blob/main/extensions/index.json)에
   항목을 추가하는 pull request를 엽니다.

```json
{
  "id": "design-report",
  "name": "Design report",
  "description": "Writes a Markdown report of a cell: devices by kind, DRC and LVS results.",
  "repository": "https://github.com/jane/ics-design-report",
  "path": "optional/sub-folder",
  "rev": "v0.1.0",
  "commit": "v0.1.0의 40자리 전체 커밋 해시",
  "license": "MIT",
  "authors": ["Jane Doe"],
  "tags": ["report", "drc"]
}
```

`commit`은 항목을 그 커밋에 고정합니다(다운로드 후 확인). 매니페스트의 id는 항목의 id와 같아야
하며, IC Studio 자체 저장소 밖의 항목은 반드시 고정해야 합니다. 설치는 `https://`와 ssh 저장소만
받으며, 앱에서 인덱스로 설치할 때는 먼저 확인을 묻습니다.

`index.json`만 바꾸는 pull request에는 CLA 서명이 필요 없습니다. 인덱스 항목 규칙은
[`extensions/README.md`](https://github.com/geniuskey/ic-studio/blob/main/extensions/README.md)에
있습니다. `ICS_EXTENSION_INDEX`로 다른 인덱스(URL이나 파일), 예를 들어 회사 내부 인덱스를 쓸 수
있습니다.
