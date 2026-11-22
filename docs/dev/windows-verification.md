# Windows verification: guide for an agent

This guide is for an agent (Claude Code) running on a real Windows machine. The project
was built on macOS. CI builds and tests it on `windows-latest`, but CI cannot check
everything: the UI there runs on WARP (software DX12), with no real GPU, no installer
run, no updater, no IME, no HiDPI and no user profile with spaces or Korean characters.
The job is to find what breaks on a real machine and fix it.

Read `CLAUDE.md` first. Its conventions apply here: PowerShell syntax, `cargo fmt`,
clippy with zero warnings, and the UI test rules (glyph coverage, labelled inputs).

## Ground rules

- **Work phase by phase, in order.** A failing build or test blocks everything after it.
  Fix it before moving on.
- **One cause, one fix.** For each failure:
  1. Reproduce it.
  2. Find the cause in the code.
  3. Fix it. Add a test where one can catch it, using `#[cfg(windows)]` when the test only
     makes sense on Windows.
  4. Run `cargo fmt --all` and clippy.
  5. Record the failure and the fix in the report.
- **Ask the user before you commit or push.** When they say so, commit straight to `main`
  (no branches, no PRs). Use one commit per fix, in the style of
  `fix(windows): <what>`, ending with the `Co-Authored-By` line.
- **Don't turn off a test or check to make it pass.** If something truly can't work on
  Windows, say so in the report and leave the decision to the user.
- **Use a scratch folder for everything you create:**
  `$env:TEMP\ics-win` and a test profile folder. Don't touch the user's real projects,
  and run uninstall at the end.
- **No separate Windows account or "isolated session" is needed.** Run every phase,
  4–6 included, in the session you are in, on the user's machine. These rules make that
  safe:
  - Projects go in the scratch folder.
  - The installer goes into a scratch `ICS_INSTALL_DIR` and is uninstalled at the end.
    Record the user `PATH` and the Start Menu folder before you install, and compare them
    after you uninstall.
  - Register MCP servers only for the scratch folder (`claude mcp add --scope local`, run
    in that folder) and remove them afterwards. Don't change the user-scope Claude Code
    configuration.
  - Close every app window and process you started.
  - When an item can't be checked from a shell (you must see a window), check it as far
    as you can first: the process starts and runs, `live.json` appears, MCP calls answer,
    and a screenshot (below) shows the window. Then list what is left under **Needs a
    human**. "Skipped" is only for items that truly can't run here, and the note says why.
- **Screenshots of the desktop:** capture the screen from PowerShell and look at the PNG:
  `Add-Type -AssemblyName System.Windows.Forms,System.Drawing;
  $b=[Windows.Forms.Screen]::PrimaryScreen.Bounds; $i=New-Object Drawing.Bitmap $b.Width,$b.Height;
  [Drawing.Graphics]::FromImage($i).CopyFromScreen($b.Location,[Drawing.Point]::Empty,$b.Size);
  $i.Save("$env:TEMP\ics-win\screen.png")`.
  The same capture taken right after a simulation starts shows whether a console window
  opened.
- **Things only the user can do** (look at a window, type with the Korean IME, click
  through SmartScreen): list exact steps in the report under **Needs a human**. Don't
  guess the outcome.

## The report

Write results to `docs/dev/windows-report.md`. If the report already exists, this is a
follow-up run:
1. `git pull` first.
2. Run only the rows that are skipped or FAIL, and the items the fixes since then touch.
3. Update those rows in place.
4. Add a line under the header giving the follow-up's date and commit. It is committed so that sessions on other
machines can read it. Use this layout:

```markdown
# Windows verification report

- Date, commit (`git rev-parse --short HEAD`), Windows version (`winver` / `[Environment]::OSVersion`),
  CPU, GPU and driver, display scaling, user profile path
- rustc / cargo versions, MSVC version, ngspice version, Python version, git version

| Phase | Item | Result | Notes |
|---|---|---|---|
| 1 | cargo test --workspace | pass / FAIL / skipped | ... |

## Failures and fixes
### <item>
Symptom (exact error text), cause (file:line), fix (commit), test added.

## Open issues
## Needs a human
```

## Phase 0: environment

```powershell
rustup show; rustc --version          # stable, x86_64-pc-windows-msvc, Rust 1.95 or newer
cargo --version; git --version
git config --get core.autocrlf        # record it (see "Line endings" below)
Get-CimInstance Win32_VideoController | Select Name, DriverVersion
Test-Path C:\Spice64\bin\ngspice_con.exe
python --version; Get-Command claude -ErrorAction SilentlyContinue
$env:USERPROFILE                       # spaces or non-ASCII characters here are worth noting
```

If MSVC is missing, ask the user to install the Visual Studio Build Tools (the "Desktop
development with C++" workload). If ngspice is missing, install it from the official
`ngspice-47_64.7z` into `C:\`, so the binary ends up at `C:\Spice64\bin\ngspice_con.exe`.
CI does the same.

## Phase 1: build and automated tests

```powershell
cargo build --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # cfg(windows) code is only linted here
cargo test --workspace
cargo test -p ics-studio --test ui -- --test-threads=1
```

- `cargo test --workspace` already includes the UI tests. Run them again with
  `--test-threads=1` because CI does, and record any difference.
- `verification_in_background` is known to be timing-flaky. If it fails, run it alone 5
  times before you call it a Windows bug.
- **Screenshots on the real GPU:**
  1. Run `$env:ICS_SHOT_DIR="$env:TEMP\ics-win\shots"; cargo test -p ics-studio --test ui`.
  2. Open the PNGs. Check that text renders (Korean included), icons are crisp, nothing is
     clipped, and there are no black or empty frames.
  3. Compare a few with the same names in `docs/screenshots/`.
- **Line endings:** there is no `.gitattributes`, so with `core.autocrlf=true` text
  fixtures are checked out with CRLF.
  - Tests that compare exact text can fail on CRLF alone. A known candidate is
    `process_edit::edits_keep_comments_and_round_trip`, which compares against
    `GENERIC180` (`include_str!`).
  - If any test fails on CRLF, decide where the bug is:
    - the code should accept CRLF input: fix the code, because users' files will have CRLF;
    - the fixture should be byte-exact: add a `.gitattributes`.
  - Write down which choice you made and why.
- Also run the ignored perf test once on a big layout and record the time:
  `$env:ICS_PERF_PROJECT=...; $env:ICS_PERF_CELL=...; cargo test --release -p ics-studio --test ui perf_ -- --ignored --nocapture`.

## Phase 2: CLI on real paths

Build once with `cargo build --release -p ics-studio -p ics-cli`, then use
`target\release\ics.exe`. Run every step below twice: in a plain path
(`$env:TEMP\ics-win\plain`) and in a hostile one. The hostile path has spaces, Korean
characters, and a total path longer than 260 characters, e.g.
`$env:TEMP\ics-win\경로 테스트\` followed by nested long folder names.

```powershell
$ics = "$PWD\target\release\ics.exe"
& $ics examples $dir                       # writes the example projects
& $ics info $dir\getting_started
& $ics sim $dir\getting_started cmos_inverter --json
& $ics netlist $dir\getting_started cmos_inverter
& $ics drc $dir\getting_started cmos_inverter
& $ics lvs $dir\getting_started cmos_inverter
& $ics gds --out $dir\inv.gds $dir\getting_started cmos_inverter
& $ics import-gds $dir\getting_started $dir\inv.gds
& $ics xsection $dir\getting_started cmos_inverter --svg $dir\x.svg
& $ics pixel --project $dir\memory_and_pixels pixels\bayer_fdti.toml   # the stack reads a layout
& $ics verify <a project with specs>
```

Cell names differ between topics. Run `ics info` to find them. For each command, check:

- The exit code (`$LASTEXITCODE`).
- No panics. No `\\?\` prefixes in printed or saved paths (`project.rs`
  `simplify_verbatim`).
- After a save, `git status` in the example folder shows no files rewritten that should
  not change.
- Projects saved on Windows open on macOS/Linux, and the other way round: no backslashes
  are stored in `ic-studio.json` or in library paths.
- `ics diff` against a git revision works, so git is found from the project folder.

## Phase 3: simulation with ngspice

- With no `ICS_NGSPICE` set, detection must find `C:\Spice64\bin\ngspice_con.exe`
  (`ics_sim::ngspice::default_exe`). If ngspice is installed somewhere else, check this
  only when you may write to `C:\Spice64`; otherwise record it as skipped with the reason.
- With no `ICS_NGSPICE` set and ngspice's `bin` folder put on `PATH` for the process
  (`$env:PATH="D:\...\Spice64\bin;$env:PATH"`), detection must pick `ngspice_con.exe`
  from the `PATH` and not the GUI build `ngspice.exe`.
- Setting `ICS_NGSPICE=C:\Spice64\bin\ngspice.exe` (the GUI build) must still run
  `ngspice_con.exe` without opening a window (`console_exe`).
- Run the same deck on the built-in engine and on ngspice. Results should agree as they
  do on macOS (see the `bandgap` demo: 1.200/1.204/1.213 V at 0/27/100 °C).
- Decks with `.include` relative paths, with CRLF line endings, and in a folder with
  spaces.
- Temp folders: `ics_core::tmp::private_dir` uses `%TEMP%`. Check that it is cleaned up
  after a run and that a run works when `%TEMP%` contains spaces or Korean characters.
  - On Unix the folder gets mode 0700. On Windows nothing is set, so record what ACL it
    gets (`icacls`).
- No console window flashes up during a simulation started from the app
  (`CREATE_NO_WINDOW`).

## Phase 4: the desktop app

```powershell
cargo run --release -- --demo
cargo run --release -- --demo --cell cmos_inverter --workspace layout
```

Start the app with `Start-Process` so the shell stays free, and stop it with
`Stop-Process` when done. The app can be driven without a person through **MCP live
mode**:

1. Start the app.
2. Check that `%LOCALAPPDATA%\ic-studio\live.json` appears.
3. Call tools through `ics mcp`. Either pipe JSON-RPC into `ics.exe mcp` straight from
   PowerShell, or register it for the scratch folder only
   (`claude mcp add --scope local ic-studio -- <path>\ics.exe mcp`). Edits show in the window, and the
   status bar shows **MCP live**.

Cover:

- Startup on the real GPU: the DX12 backend is chosen, there is no wgpu error in the
  console, and the frame rate stays smooth while panning a big layout.
  - If DX12 fails, record whether `WGPU_BACKEND=vulkan` works.
- Open, save and save-as through the native file dialogs. Overwrite conflict dialog when
  the file changed on disk.
- Autosave and recovery: make an edit, kill the process (`Stop-Process -Force`), start
  again. The recovery copy is offered from `%LOCALAPPDATA%\ic-studio`.
- Crash log: a panic writes a log there. If no command triggers a panic, skip this item
  and say so.
- Updater (`update.rs`):
  - the running exe is moved aside to `.old`, `.old1`, … and the new one is put in place;
  - leftovers are removed on the next start (`cleanup`);
  - it relaunches with the same arguments.
  - Test it with a local fake release only if that is possible without the network.
    Otherwise put it under Needs a human.
- **Settings > Add to Claude Code**: finds `claude.exe` or `claude.cmd` and registers
  `ics mcp` without a console window flashing up. This writes the user-scope
  configuration:
  1. Save the output of `claude mcp get ic-studio` (or the fact that it is absent).
  2. Run the command and check the registration.
  3. Restore the saved state.
- Process Emulation window: run, cancel, the step editor, Load… with a Windows path.

Under **Needs a human**, list: Korean IME input in text fields (cell names, search, the
TOML editor), HiDPI at 150 % and 200 %, moving the window between monitors with different
scaling, `Ctrl` shortcuts (check that none is shown as `Cmd`), copy and paste with other
apps, and drag-and-drop of a GDS file onto the window.

## Phase 5: MCP, extensions, Python

- **`ics mcp` over stdio:** nothing but JSON-RPC may reach stdout. Run a session and
  check the stream.
  - Example: pipe `initialize` + `tools/list` in and parse every line as JSON.
- **Folder confinement** (`ics-mcp/src/confine.rs`):
  - Paths with drive letters and `/` vs `\` must work.
  - Case-insensitive matches must be allowed (`C:\Proj` vs `c:\proj`).
  - These must be refused: `..`, UNC `\\server\share`, `\\?\` paths, other drive letters,
    junctions and symlinks pointing out of the folder.
  - `ICS_MCP_ROOTS` must be separated by `;`.
- **`live.json`:** on Windows it relies on `%LOCALAPPDATA%` being private. Record the ACL
  of the file (`icacls`).
  - A stale `live.json` whose port answers with something else must be cleaned up.
- **Extensions** (`ics ext ...`):
  1. Install `extensions/` examples from a folder and from git.
  2. Run a Python extension: `PYTHONPATH` joined with `;`, no console window, killed when
     the app closes.
  3. Check that `ICS_EXTENSIONS_DIR` and the default under `%APPDATA%` work.
- **Python package:** in `python/`, run `pip install maturin; maturin develop` and then
  its tests. The version must match the workspace.

## Phase 6: installer

The installer is `scripts/install.ps1`. Test it in Windows PowerShell 5.1 and in
PowerShell 7, with a throwaway folder:

```powershell
$env:ICS_INSTALL_DIR="$env:TEMP\ics-win\install"; $env:ICS_EXAMPLES_DIR="$env:TEMP\ics-win\ex"
.\scripts\install.ps1            # latest release; it checks SHA256SUMS
ic-studio.exe --version; ics.exe --version   # in a new terminal (user PATH changed)
$env:ICS_UNINSTALL="1"; .\scripts\install.ps1
```

Check:

- The Start Menu shortcut is created and removed.
- The PATH entry is added and removed.
- A bad checksum stops the install.
- The SmartScreen warning on an unsigned build is expected. Note it under Needs a human.
- Documents redirected to OneDrive works as the examples target.

## Known Windows-specific code

Start here when something fails:

| Area | Code |
|---|---|
| ngspice discovery, console build, no window | `crates/ics-sim/src/ngspice.rs` (`default_exe`, `console_exe`, `command`) |
| `\\?\` paths | `crates/ics-core/src/project.rs` (`simplify_verbatim`), `crates/ics-studio/src/update.rs` (`ics_path`) |
| Temp folders | `crates/ics-core/src/tmp.rs` |
| Self-update, `.old` files | `crates/ics-studio/src/update.rs` |
| live.json, token, per-user folder | `crates/ics-mcp/src/bridge.rs` |
| Path confinement | `crates/ics-mcp/src/confine.rs` |
| Extension processes, git | `crates/ics-mcp/src/ext/process.rs`, `ext/install.rs`, `ext/mod.rs` |
| Recovery and crash log | `crates/ics-studio/src/recovery.rs` |
| Sign-off tool discovery (`%APPDATA%`) | `crates/ics-core/src/signoff.rs` |
| Virtuoso batch (Unix-only test) | `crates/ics-core/src/virtuoso_batch.rs` |
| Installer | `scripts/install.ps1`, `scripts/fetch-examples.ps1` |
| CI on Windows | `.github/workflows/ci.yml`, `release.yml` |

## Done when

- Phases 0–6 each have a row per item in the report.
- Every failure is either fixed (with a commit and, where possible, a test) or listed
  under Open issues with its cause.
- `cargo fmt --check`, clippy and `cargo test --workspace` pass on the Windows machine
  at the final commit.
- Needs a human lists exact steps for the user.
- Then ask the user whether to commit and push the report and the fixes.
