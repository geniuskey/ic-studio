# Windows verification report

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
| 1 | Build workspace and all targets | pass | `cargo build --workspace --all-targets` completed in 53.34 s after the runner restarted. |
| 1 | Formatting check | pass | `cargo fmt --all -- --check` exited 0 after the CRLF, Git and ngspice include fixes. |
| 1 | Clippy with warnings denied | pass | `cargo clippy --workspace --all-targets -- -D warnings` exited 0 after the CRLF, Git and ngspice include fixes. |
| 1 | Workspace tests and CRLF fixtures | pass | Cargo reported 811 passed, 8 ignored, 0 failed in the follow-up run with ngspice 47. All 105 active UI tests and the ngspice correlation suite passed. |
| 1 | Serial UI suite | pass | 105 passed, 2 ignored, 0 failed in 170.89 s. Same results as the workspace run. `verification_in_background` passed in both. |
| 1 | Hardware GPU screenshots and reference comparison | pass | Vulkan on RTX 4060 Ti, with the UI test process listed by `nvidia-smi`. 105 passed, 2 ignored. Compared welcome, schematic and layout against `docs/screenshots` and inspected `copilot_korean.png`. |
| 1 | Large-layout release performance test | pass | RTX 4060 Ti with Vulkan, `hierarchy/inv_tiles` (8,192 inverter instances). Open 0.35 s, first frame 17 ms. Full view 10.7 ms/frame, zoom x8 25.4, x64 10.1, x512 10.6. |
| 2 | Release build | pass | Both desktop and CLI release targets built successfully again after the Git fix. |
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
| 2 | Cross-platform reopening | skipped | macOS and Linux were outside this Windows run. |
| 2 | Git revision diff | pass | Exit 0 against the staged Git tree in each path. The long project was inside a worktree with a short root. |
| 3 | Default console executable discovery | skipped | The built-in C: install location is absent. The D: install worked via the process-scoped `ICS_NGSPICE` override. |
| 3 | GUI executable mapping | pass | Set `ICS_NGSPICE` to the D: GUI executable. The CLI returned ngspice-47 datasets via its console sibling. |
| 3 | Built-in and ngspice correlation | pass | At 0, 27 and 100 °C, ngspice produced 1.199953, 1.204099 and 1.213332 V. Maximum bandgap output disagreement was 8.311 µV. |
| 3 | Relative includes, CRLF and spaces | pass | Reproduced the failure, fixed deck-directory resolution and quoted include parsing, then reran relative CRLF decks with and without spaces in the include filename. |
| 3 | Temporary folders, cleanup and ACL | pass | A real 100,008-point transient ran with Korean text and spaces in TEMP and TMP. Captured the live scratch folder ACL with icacls. The scratch parent was empty after exit. |
| 3 | No console window from app | skipped | The CLI exercised the shared CREATE_NO_WINDOW launch path. Desktop observation requires an isolated Windows user session. |
| 4 | Demo and layout launch | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | DX12 hardware and large-layout panning | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | MCP live desktop edits | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | File dialogs and overwrite conflict | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | Autosave and crash recovery | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | Crash log | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | Updater replacement, cleanup and relaunch | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | Claude Code registration | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 4 | Process Emulation commands | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | MCP stdio JSON-RPC | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Drive paths, case and separators | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Path confinement boundaries | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Multiple MCP roots | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Live endpoint ACL | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Stale live endpoint cleanup | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Extension folder and Git install | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Python extension environment and process lifetime | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Extension directory overrides and defaults | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 5 | Python package build, tests and version | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 6 | PowerShell 5.1 install, version and uninstall | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 6 | PowerShell 7 install, version and uninstall | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 6 | Start Menu shortcut | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 6 | User PATH | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 6 | Checksum rejection | skipped | Awaiting native desktop verification in an isolated Windows user session. |
| 6 | SmartScreen | skipped | Needs a human. |
| 6 | OneDrive Documents | skipped | Requires an isolated Windows user session with Documents redirected. |

Test fixtures and logs are under `%TEMP%\ics-win`. The hardware run used `WGPU_BACKEND=vulkan` because `egui_kittest` prefers
software adapters by default. `vulkaninfo --summary` listed only the discrete RTX
4060 Ti. The 123 PNG captures are under `%TEMP%\ics-win\shots`. Reference screenshots show
an older app version. The Windows images show newer menus, example cards, and
Ctrl shortcut labels. The Korean prompt and tool query rendered in the Copilot
capture. That capture uses a scripted test prompt, so native IME remains in
**Needs a human**.

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

## Open issues

- Default discovery at `C:\Spice64\bin\ngspice_con.exe` remains unverified. The
  confirmed D: installation was passed through `ICS_NGSPICE`.
- The desktop release resolves Documents and its persistence path through Windows
  known folders. A disposable Windows user session is needed to exercise real
  native startup and installation in isolation. Phase 5 and Phase 6 remain pending
  under the guide's sequence.
- Git for Windows cannot use a worktree root that is itself too long for its
  working-directory API. The successful long-path diff used a short worktree
  root and a deeply nested project.
- Opening a Windows-saved project on macOS or Linux remains unverified. The
  Windows run checked saved path syntax and reopened both projects.

## Needs a human

- In a disposable Windows user session, set `ICS_NGSPICE` to
  `D:\Utilities\Spice64\bin\ngspice.exe`. Start the desktop app with the demo
  and run the bandgap operating-point simulation. Check for a console window
  flash while starting the run and verify that the app shows about 1.204 V.

- In a disposable project, choose the Korean IME with Win+Space. Type and commit
  Korean text in a cell name, the search box, and the TOML editor. Verify that
  composing, committing, cancelling, and saving the text behave as expected.
- Set display scaling to 150% and then 200%. At both settings, open the layout and
  schematic workspaces and check that text, menus, and dialogs remain legible and
  fit. Move the window between monitors at different scaling and repeat.
- Open the menus and verify that Windows shortcuts display Ctrl. Try Ctrl+C and
  Ctrl+V in a text field with another Windows application, then drag a test GDS
  file from Explorer onto the app and verify that the import dialog opens.
- Launch the downloaded unsigned executable from Explorer and record whether
  SmartScreen displays a warning. If the machine redirects Documents to OneDrive,
  run the installer with the default examples location from a disposable account
  and verify the examples appear under the redirected Documents folder.
