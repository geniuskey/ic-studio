# Windows verification report

- Follow-up 2026-11-22 (KST) started at `94ac518` after `git pull --ff-only`. Final verification included the fixes through `930800e`.
- Date 2026-11-22. Starting commit `ac1be5d`.
- Windows NT 10.0.26300.0, Intel Core i5-12400F, NVIDIA GeForce RTX 4060 Ti
  (driver 32.0.16.1088), and 96 DPI (100%) recorded under the user profile
  `C:\Users\geniu`. Per-monitor scaling was not measured.
- rustc 1.97.0, cargo 1.97.0, stable `x86_64-pc-windows-msvc`, MSVC toolset
  14.44.35207 (Visual Studio 2022 Community), Python 3.13.5, Git 2.50.1.windows.1,
  PowerShell 7.6.10. Claude Code executable found. ngspice 47 was verified at
  `D:\Utilities\Spice64\bin\ngspice_con.exe` in the follow-up run.

| Phase | Item | Result | Notes |
|---|---|---|---|
| 0 | Rust, Cargo, MSVC, Git, Python, Claude Code | pass | MSVC C++ tools found through `vswhere`. |
| 0 | Windows, CPU, GPU, driver, profile and DPI | pass | CIM returned access denied. Hardware and logged profile DPI were read from the registry. |
| 0 | Git line endings | pass | `core.autocrlf=true`. The Generic180 TOML fixture was checked out as CRLF. Windows LongPathsEnabled is 1. |
| 0 | ngspice availability | pass | ngspice 47 ran from the user-supplied D: installation. The original download attempt is recorded below. |
| 1 | Build workspace and all targets | pass | `cargo build --workspace --all-targets` passed at `94ac518` and after the follow-up fixes. |
| 1 | Formatting check | pass | `cargo fmt --all -- --check` passed at `94ac518` and after the follow-up fixes. |
| 1 | Clippy with warnings denied | pass | `cargo clippy --workspace --all-targets -- -D warnings` passed at `94ac518` and after the follow-up fixes. |
| 1 | Workspace tests and CRLF fixtures | pass | 812 passed, 8 ignored, 0 failed at `94ac518` and after the follow-up fixes. All 105 active UI tests, the PATH preference test and the ngspice correlation suite passed. |
| 1 | Serial UI suite | pass | The original serial suite had 105 passed, 2 ignored, 0 failed in 170.89 s. `verification_in_background` passed in both suite runs. The follow-up reran the affected `process_emulation` test serially and passed. |
| 1 | Hardware GPU screenshots and reference comparison | pass | The original Vulkan run used the RTX 4060 Ti and had 105 passed and 2 ignored. The follow-up rerendered the process emulation and step editor PNGs. The section and doping plots matched the older reference in `docs/screenshots/tool_process_emulation.png`. |
| 1 | Large-layout release performance test | pass | RTX 4060 Ti with Vulkan, `hierarchy/inv_tiles` (8,192 inverter instances). Open 0.35 s, first frame 17 ms. Full view 10.7 ms/frame, zoom x8 25.4, x64 10.1, x512 10.6. |
| 2 | Release build | pass | Built the desktop app and CLI from the follow-up working tree. |
| 2 | Examples export | pass | All five example topics written to a 58-character plain path and a 339-character path with spaces and Korean text. |
| 2 | Project info | pass | Opened `getting_started` from both paths. |
| 2 | JSON simulation | pass | Built-in inverter run exited 0 and its stdout parsed as JSON in both paths. |
| 2 | Netlist | pass | Inverter SPICE netlist printed in both paths. |
| 2 | DRC | pass | Inverter DRC exited 0 in both paths. |
| 2 | LVS | pass | Exit 1 in both paths with the two documented demo findings, PMOS width and load capacitor. |
| 2 | GDS export | pass | Inverter written in both paths. |
| 2 | GDS import | pass | Imported the GDS as a new library and reopened both projects. |
| 2 | Cross-section SVG | pass | Wrote a nonempty SVG in both paths. |
| 2 | Pixel simulation | pass | Exit 0 in both paths using `ics pixel --project <path>/memory_and_pixels <path>/pixels/bayer_fdti.toml`. The stock TOML reads layout and needs the project flag. |
| 2 | Specs verification | pass | Bandgap passed the vref spec at all three temperatures in both paths. |
| 2 | Exit codes, displayed paths, and panics | pass | Each command returned its expected exit code. No panics or verbatim prefixes appeared in the captured output. |
| 2 | Save and Git cleanliness | pass | Only `ic-studio.json` changed among existing files. The new GDS library was added as expected. |
| 2 | Portable project paths | pass | Both saved projects reopened on Windows. No backslashes in either manifest or the new library metadata. |
| 2 | Cross-platform reopening with Linux | pass | Windows and Ubuntu under WSL opened each other's exported `getting_started` project. Used the Linux v0.2.11 CLI verified against the release SHA-256. |
| 2 | Cross-platform reopening with macOS | skipped | No macOS environment was available. |
| 2 | Git revision diff | pass | Exit 0 against the staged Git tree in each path. The long project was inside a worktree with a short root. |
| 3 | Default console executable discovery | pass | With `ICS_NGSPICE` unset and the D: location removed from process PATH, the CLI found ngspice 47 via a temporary `C:\Spice64` junction to the existing install. Removed the junction afterward. |
| 3 | Console executable discovery on PATH | pass | With the C: junction removed and `ICS_NGSPICE` unset, adding the D: bin folder to the process PATH produced ngspice-47 results. The unit test also verifies that a console executable wins over a GUI executable in an earlier directory. |
| 3 | GUI executable mapping | pass | With `ICS_NGSPICE` set to the D: GUI executable, the CLI and desktop app produced ngspice-47 results via the console sibling. |
| 3 | Built-in and ngspice correlation | pass | At 0, 27 and 100 °C, ngspice produced 1.199953, 1.204099 and 1.213332 V. Maximum bandgap output disagreement was 8.311 µV. |
| 3 | Relative includes, CRLF and spaces | pass | The CRLF deck and relative include with spaces ran through default, PATH and GUI-mapping discovery. |
| 3 | Temporary folders, cleanup and ACL | pass | A real 100,008-point transient ran with Korean text and spaces in TEMP and TMP. Captured the live scratch folder ACL with icacls. The scratch parent was empty after exit. |
| 3 | No console window from app | pass | F5 ran ngspice-47 and displayed the inverter DC sweep. A desktop capture showed the app with no console window. The live MCP bandgap run returned 1.2041 V at 27 °C. A single capture cannot rule out a brief flash. |
| 4 | Demo and layout launch | pass | Both launch forms opened native windows. The inverter layout and MCP live status rendered on the current desktop. |
| 4 | DX12 hardware and large-layout panning | pass | With `WGPU_BACKEND=dx12`, the app loaded D3D12Core and NVIDIA's DX12 driver. No wgpu errors on stderr. The 8,192-inverter layout panned with the Right arrow. The native panning frame rate was not measured. |
| 4 | MCP live desktop edits | pass | `gui_status` returned the app PID and live mode. A stdio MCP edit to 41 °C appeared in the app and was later recovered and saved. |
| 4 | File dialogs and overwrite conflict | pass | Opened a scratch project in the native folder picker, saw the conflict on Ctrl+S after a disk edit, and verified Cancel preserved the file hash. Saved the recovered edit and used Save As to a Korean path with spaces. |
| 4 | Autosave and crash recovery | pass | Waited for `recovery.json`, forcibly stopped the app, and reopened the project. Recovered and saved the 41 °C edit, then verified the copy was cleared. |
| 4 | Crash log | skipped | No normal app command triggered a panic. Forced termination exercised recovery. |
| 4 | Updater replacement and cleanup | pass | Windows swap tests covered replacement and a locked `.old` file requiring `.old1`. Native startup removed seeded `.old1` and `.new` files. |
| 4 | End-to-end updater and relaunch | pass | 2026-11-23, by the user: an installed v0.2.11 updated itself to the v0.2.12 release through Help > Check for Updates. It relaunched on the same cell and workspace, `ics` was replaced with 0.2.12 too, and the leftover `.old` files were gone on the next start. |
| 4 | Claude Code registration | pass | Local-scope registration connected and was removed. Settings > Add to Claude Code also connected through the installed `claude.exe` in a scratch `CLAUDE_CONFIG_DIR`. Removed the scratch-profile entry and verified the default config file hash. |
| 4 | Process Emulation commands | pass | Completed the generic flow, opened the step editor and loaded a TOML file with Korean text and spaces in its path. After the fix, Cancel stopped the higher-resolution run and retained the previous section. |
| 5 | MCP stdio JSON-RPC | pass | Parsed every stdout line as JSON-RPC for initialize, tools/list and tools/call. The server announced 60 built-in tools. |
| 5 | Drive paths, case and separators | pass | Opened scratch projects with drive paths, a lowercase drive and path, forward slashes, and a relative path. |
| 5 | Path confinement boundaries | pass | The rebuilt release CLI rejected parent traversal, UNC, verbatim spellings, another drive and a junction leading out of the scratch root. |
| 5 | Path confinement with symlinks | skipped | Windows refused to create a disposable symlink with WinError 1314. |
| 5 | Multiple MCP roots | pass | Opened projects in two sibling roots with `ICS_MCP_ROOTS` separated by semicolon. |
| 5 | Live endpoint ACL | pass | `icacls` on the existing default file showed SYSTEM, Administrators and the profile owner with Full Control. The live scratch file under TEMP also inherited two other entries, matching the earlier ngspice scratch ACL observation. |
| 5 | Stale live endpoint cleanup | pass | The rebuilt release CLI removed stale endpoint files in both loopback cases, invalid JSON and a close without a reply. |
| 5 | Extension folder and Git install | pass | Installed design-report from its folder and metal-frame from a local Git repository using a file URI. |
| 5 | Python extension environment and process lifetime | pass | design-report imported the bundled SDK and two marker modules from semicolon-separated directories, including a Korean path. Its live Python child had no main window and exited when the app closed. |
| 5 | Extension directory overrides and defaults | pass | `ICS_EXTENSIONS_DIR` and the default under a scratch `%APPDATA%` both contained the installed extensions. |
| 5 | Python package build, tests and version | pass | `maturin develop --locked` built the Windows abi3 wheel under Python 3.13.5 after refreshing the lockfile. All 5 pytest tests passed and package version 0.2.11 matched the workspace. |
| 6 | PowerShell 5.1 install, version and uninstall | pass | Verified the published v0.2.11 archive checksum. A fresh Windows PowerShell with the persisted PATH resolved the scratch CLI as 0.2.11. The installed desktop PE reports 0.2.11 and the rebuilt desktop `--version` printed 0.2.11 and exited 0. |
| 6 | PowerShell 7 install, version and uninstall | pass | Same release and version checks in PowerShell 7.6.10. Both shells wrote all example topics to scratch. |
| 6 | Start Menu shortcut | pass | Both installers created a shortcut targeting the scratch app and removed it on uninstall. Used a process-scoped APPDATA tree. The real Start Menu listing and original shortcut hash matched before and after. |
| 6 | User PATH | pass | Verified that new shells selected the scratch install ahead of an existing 0.2.2 install. Both uninstalls restored the raw PATH value and registry type exactly. |
| 6 | Checksum rejection | pass | The unmodified checksum-verification code rejected a locally substituted download and mismatched SHA-256 before extraction or installation. |
| 6 | SmartScreen | skipped | Needs a human to launch the downloaded build from Explorer. |
| 6 | OneDrive Documents folder resolution | pass | Windows resolved Documents to `%USERPROFILE%\OneDrive\문서` and the native picker opened there. |
| 6 | Installer writes to redirected Documents | skipped | The mapped Documents folder contains the user's synced projects. The installer used a scratch examples target. |

Follow-up fixtures and logs are under `%TEMP%\ics-win\followup-94ac518`.
Full-desktop captures were reviewed and discarded because they included other
open windows. The original test fixtures are under `%TEMP%\ics-win`.
The original hardware run used `WGPU_BACKEND=vulkan` because `egui_kittest` prefers
software adapters by default. `vulkaninfo --summary` listed only the discrete RTX
4060 Ti. The 123 PNG captures are under `%TEMP%\ics-win\shots`. Reference screenshots show
an older app version. The Windows images show newer menus, example cards, and
Ctrl shortcut labels. The Korean prompt and tool query rendered in the Copilot
capture. That capture uses a scripted test prompt, so native IME remains in
**Needs a human**.

The installer and WSL checks used the published v0.2.11 assets.
[IC Studio v0.2.11 release](https://github.com/geniuskey/ic-studio/releases/tag/v0.2.11)

## Failures and fixes

### ngspice setup

`curl: (7) Failed to connect to downloads.sourceforge.net:443 over proxy 127.0.0.1 after 4 ms: Could not connect to server`.
Initially, with `ICS_NGSPICE` unset, the release CLI reported
`error: failed to launch 'ngspice': program not found. Install ngspice or set its path in Settings.`
The user supplied an existing D: installation and the follow-up used a
process-scoped override. The canonical C: install was outside the runner's
writable folders. [Official Windows archive](https://sourceforge.net/projects/ngspice/files/ng-spice-rework/47/ngspice-47_64.7z/download)

### Process flow edits rewrite CRLF

`process_edit::tests::edits_keep_comments_and_round_trip` failed at the first
byte-for-byte comparison (`crates/ics-core/src/process_edit.rs:330` before
the fix) with ``assertion `left == right` failed``. The left-hand output had
`\n` and the original fixture on the right had `\r\n`. The failure reproduced
when the test ran alone. `toml_edit` drops carriage returns during serialization.
The process editor now follows the first line ending in the input for all five
edit operations, including files using CRLF outside the repository. It normalizes
mixed line endings to that first style. Regression test
`edits_preserve_crlf_line_endings` checks no-op edits, changes to header and step
fields, and inserting, moving, and removing steps. Fixed in `77634b4`.

### Git revision lookup in long paths and Korean paths

The release CLI initially exited 1 with `fatal: cannot change to '<project>':
Filename too long`. Git for Windows 2.50.1 also refused the same working
directory with `core.longpaths=true` and with a verbatim path. A separate
short Korean path reproduced Git's escaped filenames in the error
`fatal: path '"\352\262\275..."' does not exist in '<tree>'`.

`crates/ics-core/src/layout_diff.rs` now runs Git from the containing worktree
root for long Windows project paths and passes the project-relative path as a
pathspec. Git tree listings use NUL delimiters to return unquoted pathnames.
`project_at_git_revision_in_long_korean_path` stages a project in a long Korean
path and opens the resulting tree. The focused layout-diff tests passed
(5 passed). Fixed in `cd4fb22`.

### Relative ngspice include paths

Running `ics spice deck.cir --backend ngspice` from a folder containing the
deck failed with `Error: Could not find include file nested.inc`. An empty or
relative deck directory left the include relative when ngspice moved into its
scratch working directory. `crates/ics-sim/src/lib.rs` now resolves the deck
directory before rewriting paths. Rewriting and include validation share the
quoted-file parser in `crates/ics-sim/src/ngspice.rs`. Added coverage for empty
and relative directories and extended the existing validation test to use a
library filename with spaces. Fixed in `91ae094`.

The observed Windows scratch ACL was inherited. SYSTEM, Administrators and
the profile owner had Full Control. The two additional inherited principals
had Modify and Read/Execute plus Write, respectively.

### Desktop launch and test paths

The app used Windows known folders for persistence and automatic example
seeding. On this machine those paths include existing IC Studio data and
OneDrive Documents. `ICS_SETTINGS_PATH` now lets an app process use a separate
eframe settings file, and startup honors the installer's `ICS_EXAMPLES_DIR`
(`crates/ics-studio/src/main.rs`, `app.rs`). The follow-up used both, plus the
existing live and recovery directory overrides. Fixed in `1c78e41`. Desktop `--version` previously
fell through to the project-path argument. The rebuilt executable printed
`ic-studio 0.2.11` and exited 0 with redirected stdout (`lib.rs`). Fixed in `2be994b`.

### Windows MCP path spellings

Real stdio `open_project` calls accepted both an in-root path with `..` and a
verbatim path. Confinement checked only the resolved target. The Windows
argument validation in `crates/ics-mcp/src/confine.rs` now checks those spellings
before canonicalization. Added Windows coverage for allowed drive paths and
rejected spellings. The existing RPC tests still exercise the ordinary
out-of-root error and use plain paths to their fixtures. Fixed in `c266a35`.

### Foreign listener closes without a reply

On a loopback test port, a listener read the bridge ping and closed with no
response. Discovery fell back to standalone but left `live.json` behind.
`crates/ics-mcp/src/bridge.rs` now treats EOF during the ping as stale while
retaining the file on timeout. Added a regression case to the existing stale
endpoint test. Fixed in `95fb79f`.

### Process Emulation Cancel button moves

On a running 36-step flow, Cancel shifted as step names changed. Two clicks
landed outside the button after it moved. `crates/ics-studio/src/process_view.rs`
now lays out Cancel before the changing text. The rebuilt app displayed
`Cancelling…`, stopped the run and kept the previous section. Fixed in `7a28a15`.

### New install was shadowed by an older CLI

Both Windows shells found the existing `ics 0.2.2` after the scratch installer
reported success. `scripts/install.ps1` appended its folder to the user and
process PATH. It now prepends a new entry. Reran both installers and resolved
`ics 0.2.11` from the scratch path in a fresh process using the persisted
machine and user PATH. The old version still resolved after each uninstall. Fixed in `5a13e31`.

### Python lockfile was stale

`maturin develop` refreshed the separate Python lockfile for the workspace's
existing `toml_edit` dependency. The maturin build used the workspace crate
versions and passed the Python tests. Lockfile refreshed in `930800e`.

## Open issues

- Git for Windows cannot use a worktree root that is itself too long for its
  working-directory API. The successful long-path diff used a short worktree
  root and a deeply nested project.
- A macOS round trip remains unverified. Linux under WSL verified both directions.
- The runner could create and test a directory junction but lacked permission
  to create a Windows symlink (WinError 1314).
- Console captures are point-in-time and do not prove that a short flash never
  occurred. The ngspice and Python paths use `CREATE_NO_WINDOW`.

## Needs a human

- In a scratch project, select the Korean IME with Win+Space. Compose, commit,
  cancel and save Korean text in a cell name, the search box and the TOML editor.
- At 150% and 200% Windows display scaling, open the schematic and layout.
  Verify text, menus and dialogs fit. Move the window between monitors with
  different scaling and repeat if both monitors are available.
- With the scratch app open, check Ctrl+C and Ctrl+V between an app text field
  and another Windows application. Drag a scratch GDS file from Explorer onto
  the app and verify the import dialog opens. Menu labels and Ctrl+S were
  exercised during the follow-up.
- Launch an unsigned downloaded build from Explorer and record the SmartScreen
  behavior.
- To check for a brief console flash, run the scratch inverter with ngspice
  using F5, launch a Python extension, and select Settings > Add to Claude
  Code with a scratch Claude profile. Observe the desktop during each launch.
- To check the OneDrive installer default on a profile with Documents redirected,
  use an empty test Documents location, unset `ICS_EXAMPLES_DIR`, install with
  a scratch `ICS_INSTALL_DIR`, and verify the examples appear under
  `Documents\IC Studio\Examples` before uninstalling.

## OTA trial follow-up

The [participant task](ota5t-trial-task.ko.md) and [observation sheet](ota5t-trial-observations.ko.md)
use a scratch copy of the analog example with an AC stop frequency of 30 GHz. The CLI
preflight measured 5.43 GHz for the original and 11.40 GHz for the derived cell at 27 °C.
The derived cell passed the three-temperature gain, UGF and phase-margin checks, DRC and LVS.
The original schematic's hash matched that of a fresh fixture.

Three scratch decks also ran on both the built-in engine and ngspice 47. The JFET
deck used series resistances, gate capacitances and a non-default doping parameter.
Its largest drain-voltage difference was 9.22 nV in AC. The matched line differed by
at most 1.13 mV on its delayed 0.5 V pulse. The `POLY` and `VALUE` output voltages
agreed within 1.23 nV in OP and AC. **Auto** selected the built-in engine for all
three decks.

The four-thread workspace run passed 827 tests with 9 ignored, including all 106 active
UI tests. In the first workspace run, `layout_compare_saved` did not reach its compare
view in the three frames the test allows. It passed by itself, in the four-thread UI
run, and in the four-thread workspace run.

| Check | Evidence | Status |
| --- | --- | --- |
| Desktop OTA task | Native Windows automation found the wizard action clipped below the visible scroll area. The action now sits above that area, and the target inputs have a minimum width. The UI regression test starts the action without scrolling. One novice participant then used the revised app to derive the new cell with step-by-step guidance. The live API assisted with saving. | Automated test passed. Participant result verified on disk at 11.402 GHz against 5.430 GHz. |
| GDS/OASIS drop | The application lacked a handler for native file-drop events. It now shows an import dialog for supported extensions and uses the existing importer after confirmation. The UI test sent a drop event, confirmed import, and checked the resulting cell layouts. | Automated test passed. Native Explorer drag pending. |
| IME, HiDPI and clipboard | The Windows profile had a Korean IME installed and the isolated Korean UI rendering test passed. Computer Use stopped on physical Escape before native input and scaling interactions. | Pending native input and display checks. |
| SmartScreen | Verified the official v0.2.11 Windows executable against the published SHA-256. Windows reported `NotSigned`. The scratch file has an Internet-zone marker. | Explorer launch and prompt observation pending. |

[Published v0.2.11 assets and hashes](https://github.com/geniuskey/ic-studio/releases/expanded_assets/v0.2.11)
