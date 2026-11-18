<!-- seo title="Extensions" description="Extend IC Studio with third-party extensions: tools, menu commands, SKILL scripts and PDKs, written in any language, installed from git or the community index." -->

# Extensions

Extensions add features to IC Studio without changing IC Studio itself. Anyone can write one,
publish it in their own git repository and list it in the community index. An extension can
add:

- **tools**: functions served by the extension's own program, written in any language (Python
  is the easiest). They work everywhere the built-in tools work: the **Extensions** menu, the
  command palette, the Design Copilot (**Tools > Design Copilot**), AI agents through the [MCP server](mcp.md),
  the [command line](cli.md) and the [Python API](python.md). While a tool runs, its program can
  call every built-in tool on the open project: read netlists, simulate, run DRC and LVS, draw
  layout.
- **menu commands**: an entry under **Extensions** that runs a tool (its own or a built-in one)
  or a Cadence SKILL script on the active cell.
- **PDKs**: [PDK files](technology.md) offered under **Extensions > Load PDK**.

An extension is a folder with an `extension.toml`. Extensions with only SKILL scripts or PDK
files need no program at all.

> Extensions with a program run it with your rights, like any program you download. Install
> only extensions you trust, and read the code of ones you are not sure about.

## Using extensions

### In the app

**Extensions > Manage Extensions…** lists the installed extensions with their tools, commands,
PDKs, the program they run and where they came from. There you can:

- switch an extension off and on (**Enabled**),
- **Install from Folder…**, or **Install from Git** with a repository URL (optionally a
  sub-folder and a tag or branch),
- **Load Index** under **Community index**, search it and **Install** one,
- **Update** an extension from where it was installed, **Remove** it, open its folder,
  website, or **Stop** its running program (it starts again when needed).

Each enabled extension with commands gets a submenu under **Extensions**; its commands are in
the command palette too (⌘K / Ctrl+K). Edits an extension makes are undoable like any other
edit, and its log lines appear in the console.

The Design Copilot sees every enabled extension's tools, so you can ask it, for example, to
"write a design report of nand2" when the design-report extension is installed.

### On the command line

```sh
ics ext search                      # the community index
ics ext install design-report       # by index id
ics ext install https://github.com/you/ics-thing --rev v1.0   # any git repository
ics ext install ./my-extension      # a local folder (copied)
ics ext list                        # installed: tools, commands, PDKs, state
ics ext disable design-report       # enable / disable / update / remove <id>
ics ext call myproj design-report__report '{"cell": "nand2"}'
ics ext run myproj metal-frame/frame --cell nand2
```

`ics ext call` runs any tool, an extension's or a built-in one, on a saved project and saves
the project when the tool changed it (`--no-save` to keep it unchanged). `ics ext run` runs a
menu command, `<extension id>/<command id>`, with `${cell}` set by `--cell`.

Installing from git needs `git`; the index is downloaded with `curl` (both come with macOS,
Linux and Windows 10 or newer, git on Windows with Git for Windows).

### Tool names

An extension's tools are named `<extension id>__<tool>` (two underscores), e.g.
`design-report__report`, everywhere: in `ics mcp`'s tool list, the copilot, `ics ext call` and
`Project.call` in Python.

### Where extensions live

Installed extensions are folders in the per-user extensions folder:

| System | Folder |
|---|---|
| Windows | `%APPDATA%\ic-studio\extensions` |
| macOS | `~/Library/Application Support/ic-studio/extensions` |
| Linux | `$XDG_DATA_HOME/ic-studio/extensions` (`~/.local/share/ic-studio/extensions`) |

`ICS_EXTENSIONS_DIR` sets another folder (for example one shared by a team), and
`ICS_NO_EXTENSIONS=1` starts `ics mcp` without any. After copying an extension folder there by
hand, use **Reload** in the Extensions window or **Reload extensions** in the command palette.

## Writing an extension

Start from a template:

```sh
ics ext new my-tool                      # Python program with one tool and a menu command
ics ext new my-script --template skill   # a SKILL script as a menu command, no program
ics ext check my-tool                    # validate extension.toml
ics ext install my-tool                  # copy it into the extensions folder
```

While developing, install again with `ics ext install my-tool --force` after a change, or set
`ICS_EXTENSIONS_DIR` to the folder that holds your work so nothing needs copying.

Two complete examples are in the repository's
[`extensions/`](https://github.com/geniuskey/ic-studio/tree/main/extensions) folder:
`design-report` (Python: counts devices, runs DRC and LVS through IC Studio, writes Markdown)
and `metal-frame` (one SKILL script).

### extension.toml

```toml
[extension]
id = "design-report"              # lowercase letters, digits and '-'; unique in the index
name = "Design report"
version = "0.1.0"                 # x.y.z
description = "Writes a Markdown report of a cell: devices, DRC and LVS."
authors = ["Jane Doe"]
license = "MIT"
repository = "https://github.com/jane/ics-design-report"
ics = ">=0.2.9"                   # IC Studio versions it works with

[run]                             # the program serving the tools
command = ["python3", "main.py"]  # relative to the extension folder, else on the PATH
windows = ["python", "main.py"]   # optional: used on Windows instead of command
env = { MY_SETTING = "1" }        # optional extra environment

[[tools]]
name = "report"                   # becomes design-report__report
description = "Check a cell and write reports/<cell>.md."
read_only = false                 # true: the copilot may use it in plan mode
input_schema = { type = "object", properties = { cell = { type = "string" } }, required = ["cell"] }

[[commands]]
id = "report"
title = "Design report of the active cell"
tool = "report"                   # its own tool, or a built-in tool such as run_drc
arguments = { cell = "${cell}" }

[[commands]]
id = "frame"
title = "Draw a metal frame"
skill = "scripts/frame.il"        # a SKILL script, run on the active cell

[[pdks]]
name = "My 130 nm PDK"
file = "pdk/my130.toml"
```

| Key | Meaning |
|---|---|
| `extension.ics` | Comma-separated version comparators (`>=`, `>`, `<=`, `<`, `=`); a bare version means `>=`. An extension for another version is listed but not used. |
| `run.command` | Program and arguments. `${extension}` in arguments and `env` values is the extension folder. |
| `tools[].description` | What the tool does, for people and for AI agents choosing it: say what it needs and what it returns. |
| `tools[].input_schema` | JSON Schema of the arguments (written as TOML); without it the tool takes none. |
| `commands[].arguments` | Fixed arguments. In strings, `${cell}` is the active cell (`lib/cell`), `${project}` the project folder and `${extension}` the extension folder. A command that needs `${cell}` asks for an open cell. |
| `commands[].skill` | A SKILL file in the extension, run like the [`run_skill`](mcp.md#run_skill) tool on the active cell: files only under the project folder, no environment, at most 120 s; undoable. |

### A tool in Python

IC Studio puts its helper module `ics_extension` (standard library only) on `PYTHONPATH` when
it starts the program, so nothing needs installing:

```python
from ics_extension import Extension, ToolError

ext = Extension()

@ext.tool("report")
def report(cell, write=True):
    deck = ext.call("get_netlist", cell=cell)["deck"]   # any built-in tool
    drc = ext.call("run_drc", cell=cell)
    ext.log(f"{cell}: {drc['violations']} DRC violations")  # to IC Studio's console
    return {"cell": cell, "violations": drc["violations"],
            "message": f"{cell}: DRC {drc['violations']}"}   # shown in the console

if __name__ == "__main__":
    ext.run()
```

- A tool returns a JSON value (a dict, list, string or number). Raise an exception to report an
  error; the program keeps serving the next calls.
- `ext.call(tool, **arguments)` runs a built-in tool on the project open in the window (or the
  one `ics ext call` / `ics mcp` opened) and returns its result; a failing tool raises
  `ToolError`. The tools and their arguments are those of the [MCP server](mcp.md#tools).
  Calling another extension's tool is not allowed.
- Never print to stdout: it carries the protocol. `print` is redirected to stderr, which
  IC Studio shows when the program fails.
- If your result has a `"message"` string, the console shows it when the tool runs from the
  menu.
- The program starts on the first call of one of its tools and keeps running, so it can keep
  state (a loaded model, a cache) between calls.

`ics_extension.py` is MIT licensed: copy it into your extension if you prefer.

### Any other language

The program talks newline-delimited [JSON-RPC 2.0](https://www.jsonrpc.org/specification) on
stdin and stdout; stderr is its log. The messages are a subset of the
[Model Context Protocol](https://modelcontextprotocol.io), so an existing MCP server that serves
tools works as an extension as it is (it just cannot call IC Studio back).

IC Studio to the program:

| Message | Meaning |
|---|---|
| `initialize` (request) | First message. `params`: `protocolVersion` (MCP's), `icsProtocol` (this protocol, `1`), `clientInfo` / `hostInfo` `{name, version}`, `extension` `{id, dir}`. Answer with any result, e.g. `{"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}}`. |
| `notifications/initialized` | After the answer to `initialize`. |
| `tools/call` (request) | `{name, arguments}`: run a tool (the `name` from `extension.toml`, without the id). Answer `{content: [{type: "text", text}], isError}`; any other result is taken as the tool's JSON value. |
| `shutdown` (notification) | Exit now; stdin closes next. |

The program to IC Studio, while it is answering a `tools/call`:

| Message | Meaning |
|---|---|
| `ics/call` (request) | `{name, arguments}`: run a built-in tool on the open project. The answer has the `tools/call` form; the text is the tool's JSON result. |
| `ics/log` (notification) | `{level, message}`: a console line (`info`, `warning`, `error`). |

Environment: `ICS_EXTENSION_ID`, `ICS_EXTENSION_DIR`, `ICS_VERSION`; the working directory is
the extension folder. A call that takes longer than 30 minutes stops the program.

A minimal exchange (`>` to the program, `<` from it):

```text
> {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","icsProtocol":1,...}}
< {"jsonrpc":"2.0","id":1,"result":{}}
> {"jsonrpc":"2.0","method":"notifications/initialized"}
> {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"report","arguments":{"cell":"nand2"}}}
< {"jsonrpc":"2.0","id":"a","method":"ics/call","params":{"name":"run_drc","arguments":{"cell":"nand2"}}}
> {"jsonrpc":"2.0","id":"a","result":{"content":[{"type":"text","text":"{\"clean\":true,...}"}],"isError":false}}
< {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"ok\":true}"}],"isError":false}}
```

### Publishing

1. Put the extension in a public git repository (the repository root, or a sub-folder) with a
   README and a license. Tag releases (`v0.1.0`) so users can pin one.
2. Check it with `ics ext check` and install it from the repository URL once.
3. Add an entry to
   [`extensions/index.json`](https://github.com/geniuskey/ic-studio/blob/main/extensions/index.json)
   in a pull request:

```json
{
  "id": "design-report",
  "name": "Design report",
  "description": "Writes a Markdown report of a cell: devices by kind, DRC and LVS results.",
  "repository": "https://github.com/jane/ics-design-report",
  "path": "optional/sub-folder",
  "rev": "v0.1.0",
  "license": "MIT",
  "authors": ["Jane Doe"],
  "tags": ["report", "drc"]
}
```

A pull request that changes only `index.json` needs no CLA. The rules for index entries are in
[`extensions/README.md`](https://github.com/geniuskey/ic-studio/blob/main/extensions/README.md).
`ICS_EXTENSION_INDEX` points IC Studio at another index (a URL or a file), for example a
company's internal one.
