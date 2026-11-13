# IC Studio installer for Windows (Windows PowerShell 5.1 or PowerShell 7).
#
#   irm https://github.com/geniuskey/ic-studio/releases/latest/download/install.ps1 | iex
#
# Installs the app (ic-studio.exe) and the command line tool (ics.exe, CLI and MCP server)
# from a GitHub release into one folder, adds it to the user PATH, creates a Start Menu
# shortcut and registers the MCP server (ics mcp) with Claude Code when `claude` is on the PATH.
#
# Options (environment variables):
#   ICS_VERSION      release tag to install, e.g. v0.2.0 (default: latest)
#   ICS_INSTALL_DIR  install folder (default: %LOCALAPPDATA%\Programs\IC Studio)
#   ICS_UNINSTALL=1  remove what this script installed

& {
    $ErrorActionPreference = 'Stop'
    $Repo = 'geniuskey/ic-studio'
    $Platform = 'windows-x86_64'
    $Exes = @('ic-studio.exe', 'ics.exe')

    $Dir = if ($env:ICS_INSTALL_DIR) { $env:ICS_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\IC Studio' }
    $Dir = [System.IO.Path]::GetFullPath($Dir)
    $AppExe = Join-Path $Dir 'ic-studio.exe'
    $IcsExe = Join-Path $Dir 'ics.exe'
    $Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\IC Studio.lnk'

    function Write-Step([string]$Message) { Write-Host "==> $Message" -ForegroundColor Cyan }

    function Test-Claude { [bool](Get-Command claude -ErrorAction SilentlyContinue) }

    function Invoke-Claude([string[]]$Arguments) {
        $saved = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        try {
            & claude @Arguments 2>&1 | Out-Null
            return ($LASTEXITCODE -eq 0)
        } catch {
            return $false
        } finally {
            $ErrorActionPreference = $saved
        }
    }

    function Get-RunningApp {
        Get-Process -ErrorAction SilentlyContinue | Where-Object {
            $_.ProcessName -in @('ic-studio', 'ics') -and $_.Path -and
            ([System.IO.Path]::GetDirectoryName($_.Path) -ieq $Dir)
        }
    }

    function Send-EnvironmentChange {
        if (-not ('IcsInstaller.Native' -as [type])) {
            Add-Type -Namespace IcsInstaller -Name Native -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
        }
        $result = [UIntPtr]::Zero
        [void][IcsInstaller.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
    }

    function Get-UserPath {
        $key = Get-Item -Path 'HKCU:\Environment'
        [string]$key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
    }

    function Set-UserPath([string]$Value) {
        Set-ItemProperty -Path 'HKCU:\Environment' -Name 'Path' -Value $Value -Type ExpandString
        Send-EnvironmentChange
    }

    function Test-SameDir([string]$Entry, [string]$Target) {
        $expanded = [Environment]::ExpandEnvironmentVariables($Entry).Trim().TrimEnd('\')
        return ($expanded -ieq $Target.TrimEnd('\'))
    }

    function Stop-IfRunning {
        $running = @(Get-RunningApp)
        if ($running.Count -gt 0) {
            Write-Host ''
            Write-Host "IC Studio is running from $Dir (process: $(($running | ForEach-Object { $_.ProcessName } | Sort-Object -Unique) -join ', '))." -ForegroundColor Yellow
            Write-Host 'Close the app and any MCP client using it (e.g. Claude Code), then run the installer again.' -ForegroundColor Yellow
            return $true
        }
        return $false
    }

    if ($env:ICS_UNINSTALL -eq '1') {
        if (Stop-IfRunning) { return }
        Write-Step 'Uninstalling IC Studio'
        if (Test-Path -LiteralPath $Dir) {
            Remove-Item -LiteralPath $Dir -Recurse -Force
            Write-Host "Removed $Dir"
        }
        $userPath = Get-UserPath
        $entries = @($userPath -split ';' | Where-Object { $_ -ne '' })
        $kept = @($entries | Where-Object { -not (Test-SameDir $_ $Dir) })
        if ($kept.Count -ne $entries.Count) {
            Set-UserPath ($kept -join ';')
            Write-Host "Removed $Dir from the user PATH"
        }
        if (Test-Path -LiteralPath $Shortcut) {
            Remove-Item -LiteralPath $Shortcut -Force
            Write-Host 'Removed the Start Menu shortcut'
        }
        if (Test-Claude) {
            if (Invoke-Claude @('mcp', 'remove', '--scope', 'user', 'ic-studio')) {
                Write-Host 'Removed the ic-studio MCP server from Claude Code'
            }
        }
        Write-Host 'IC Studio is uninstalled. Projects and settings are kept.'
        return
    }

    if ($env:PROCESSOR_ARCHITECTURE -ne 'AMD64' -and $env:PROCESSOR_ARCHITEW6432 -ne 'AMD64') {
        if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') {
            Write-Host 'Note: no Windows ARM64 build is published; installing the x86_64 build (runs under emulation).' -ForegroundColor Yellow
        } else {
            Write-Host "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE. Only 64-bit Windows is supported." -ForegroundColor Red
            return
        }
    }

    $asset ="ic-studio-$Platform.zip"
    $url = if ($env:ICS_VERSION) {
        "https://github.com/$Repo/releases/download/$($env:ICS_VERSION)/$asset"
    } else {
        "https://github.com/$Repo/releases/latest/download/$asset"
    }

    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ic-studio-install-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    $savedProgress = $ProgressPreference
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
        $ProgressPreference = 'SilentlyContinue'

        Write-Step "Downloading $url"
        $zip = Join-Path $tmp $asset
        try {
            Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing
        } catch {
            Write-Host "Download failed: $($_.Exception.Message)" -ForegroundColor Red
            if ($env:ICS_VERSION) { Write-Host "Check that release $($env:ICS_VERSION) exists and has $asset." -ForegroundColor Red }
            return
        }

        Write-Step 'Extracting'
        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive -LiteralPath $zip -DestinationPath $unpacked -Force
        foreach ($exe in $Exes) {
            if (-not (Test-Path -LiteralPath (Join-Path $unpacked $exe))) {
                Write-Host "The archive does not contain $exe." -ForegroundColor Red
                return
            }
        }

        Write-Step "Installing to $Dir"
        New-Item -ItemType Directory -Path $Dir -Force | Out-Null
        foreach ($exe in $Exes) {
            # A running exe (the app, or an MCP client's ics mcp) cannot be overwritten but
            # can be renamed; IC Studio deletes the .old files on its next start.
            $target = Join-Path $Dir $exe
            if (Test-Path -LiteralPath $target) {
                Remove-Item -LiteralPath "$target.old" -Force -ErrorAction SilentlyContinue
                Move-Item -LiteralPath $target -Destination "$target.old" -Force
            }
            Copy-Item -LiteralPath (Join-Path $unpacked $exe) -Destination $target -Force
        }
        if (@(Get-RunningApp).Count -gt 0) {
            Write-Host 'IC Studio is running; restart it (and MCP clients such as Claude Code) to use the new version.' -ForegroundColor Yellow
        }
    } finally {
        $ProgressPreference = $savedProgress
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
    if (-not (Test-Path -LiteralPath $AppExe)) { return }

    $userPath = Get-UserPath
    $entries = @($userPath -split ';' | Where-Object { $_ -ne '' })
    $pathAdded = $false
    if (-not ($entries | Where-Object { Test-SameDir $_ $Dir })) {
        Set-UserPath ((@($entries) + $Dir) -join ';')
        $pathAdded = $true
    }
    if (-not (($env:Path -split ';') | Where-Object { $_ -and (Test-SameDir $_ $Dir) })) {
        $env:Path = ($env:Path.TrimEnd(';') + ';' + $Dir)
    }

    Write-Step 'Creating the Start Menu shortcut'
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut($Shortcut)
    $link.TargetPath = $AppExe
    $link.WorkingDirectory = $env:USERPROFILE
    $link.IconLocation = "$AppExe,0"
    $link.Description = 'IC Studio'
    $link.Save()

    $mcpStatus = ''
    if (Test-Claude) {
        Write-Step 'Registering the MCP server with Claude Code'
        [void](Invoke-Claude @('mcp', 'remove', '--scope', 'user', 'ic-studio'))
        if (Invoke-Claude @('mcp', 'add', '--scope', 'user', 'ic-studio', '--', $IcsExe, 'mcp')) {
            $mcpStatus = 'registered with Claude Code as "ic-studio" (user scope)'
        } else {
            $mcpStatus = 'registration with Claude Code failed'
        }
    }

    $jsonPath = $IcsExe.Replace('\', '\\')
    Write-Host ''
    Write-Host 'IC Studio is installed.' -ForegroundColor Green
    Write-Host "  Folder:   $Dir"
    Write-Host '  App:      ic-studio   (or Start Menu > IC Studio)'
    Write-Host '  CLI:      ics --help   (MCP server: ics mcp)'
    if ($mcpStatus) {
        Write-Host "  MCP:      $mcpStatus"
    }
    if (-not $mcpStatus -or $mcpStatus -like '*failed*') {
        Write-Host '  MCP:      to register with Claude Code, run'
        Write-Host "              claude mcp add --scope user ic-studio -- `"$IcsExe`" mcp"
        Write-Host '            for other MCP clients, add'
        Write-Host "              { `"mcpServers`": { `"ic-studio`": { `"command`": `"$jsonPath`", `"args`": [`"mcp`"] } } }"
    }
    if ($pathAdded) {
        Write-Host "  PATH:     added $Dir to the user PATH; open a new terminal to use it there."
    }
}
