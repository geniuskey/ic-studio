<#
.SYNOPSIS
Downloads open-source SKY130 and GF180MCU designs and builds ready-to-open IC Studio projects.

.DESCRIPTION
Creates examples/sky130 (caravel_clocking, housekeeping, chip_io; optionally the full caravel
chip) and examples/gf180 (7-track standard cells and a 512x8 SRAM macro), each with the PDK's
KLayout layer set imported as its technology. Downloads land in examples/downloads and are
reused; libraries already in a project are not imported again.

The `ics` CLI is taken from $env:ICS, target/release, or `cargo run --release -p ics-cli`.

.EXAMPLE
./scripts/fetch-examples.ps1
./scripts/fetch-examples.ps1 -Caravel     # also caravel.gds (52 MB download, 278 MB unpacked)
./scripts/fetch-examples.ps1 -Force       # rebuild the projects from the downloads
#>
param(
    [switch]$Caravel,
    [switch]$Force,
    [string]$Dest = (Join-Path (Split-Path $PSScriptRoot -Parent) 'examples')
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$Repo = Split-Path $PSScriptRoot -Parent

$Ics = if ($env:ICS) {
    , $env:ICS
} else {
    $bin = @('ics.exe', 'ics') | ForEach-Object { Join-Path $Repo "target/release/$_" } | Where-Object { Test-Path $_ -PathType Leaf } | Select-Object -First 1
    if ($bin) { , $bin } else { @('cargo', 'run', '--quiet', '--release', '-p', 'ics-cli', '--manifest-path', (Join-Path $Repo 'Cargo.toml'), '--') }
}

function Invoke-Ics {
    $exe, $rest = $Ics
    & $exe @rest @args
    if ($LASTEXITCODE -ne 0) { throw "ics $($args -join ' ') failed (exit code $LASTEXITCODE)" }
}

function Get-Download([string]$Url, [string]$Out) {
    if (Test-Path $Out) {
        Write-Host "  have   $(Split-Path $Out -Leaf)"
        return
    }
    New-Item -ItemType Directory -Force (Split-Path $Out -Parent) | Out-Null
    Write-Host "  fetch  $Url"
    $part = "$Out.part"
    try {
        Invoke-WebRequest -Uri $Url -OutFile $part -UseBasicParsing
    } catch {
        Remove-Item $part -ErrorAction SilentlyContinue
        throw "download failed: $Url`n  $($_.Exception.Message)`n  Check the network connection and run the script again; finished downloads are kept."
    }
    Move-Item $part $Out
    Write-Host ("         {0:N1} MB" -f ((Get-Item $Out).Length / 1MB))
}

function Expand-Gzip([string]$Gz, [string]$Out) {
    if (Test-Path $Out) { return }
    Write-Host "  unpack $(Split-Path $Gz -Leaf)"
    $in = [IO.File]::OpenRead($Gz)
    try {
        $zip = New-Object IO.Compression.GZipStream($in, [IO.Compression.CompressionMode]::Decompress)
        $file = [IO.File]::Create("$Out.part")
        try { $zip.CopyTo($file) } finally { $file.Dispose(); $zip.Dispose() }
    } finally {
        $in.Dispose()
    }
    Move-Item "$Out.part" $Out
}

# Downloads `$Base/<file>` for every file into `$Dir`, unpacking .gz; returns lib name -> GDS path.
function Get-Designs([string]$Base, [string]$Dir, [string[]]$Files) {
    $libs = [ordered]@{}
    foreach ($f in $Files) {
        $out = Join-Path $Dir $f
        Get-Download "$Base/$f" $out
        if ($f.EndsWith('.gz')) {
            $gds = $out.Substring(0, $out.Length - 3)
            Expand-Gzip $out $gds
            $out = $gds
        }
        $libs[[IO.Path]::GetFileNameWithoutExtension($out)] = $out
    }
    $libs
}

function Build-Project([string]$Dir, [string]$Lyp, [string]$Lyt, $Libs) {
    Write-Host "project $Dir"
    if ($Force -and (Test-Path $Dir)) { Remove-Item -Recurse -Force $Dir }
    if (-not (Test-Path (Join-Path $Dir 'ic-studio.json'))) {
        Invoke-Ics new $Dir --template empty
        Invoke-Ics import-tech $Dir $Lyp --lyt $Lyt
    }
    $have = Invoke-Ics info $Dir | ForEach-Object { if ($_ -match '^\s+(\S+)/$') { $Matches[1] } }
    foreach ($lib in $Libs.Keys) {
        if ($have -contains $lib) {
            Write-Host "  library $lib already imported"
            continue
        }
        Write-Host "  import $lib"
        Invoke-Ics import-gds $Dir $Libs[$lib] --lib $lib
    }
}

$downloads = Join-Path $Dest 'downloads'

Write-Host '== SKY130 (efabless caravel)'
$skyTech = 'https://raw.githubusercontent.com/RTimothyEdwards/open_pdks/master/sky130/klayout'
$skyDir = Join-Path $downloads 'sky130'
Get-Download "$skyTech/sky130.lyp" (Join-Path $skyDir 'sky130.lyp')
Get-Download "$skyTech/sky130.lyt" (Join-Path $skyDir 'sky130.lyt')
$skyFiles = @('caravel_clocking.gds.gz', 'housekeeping.gds.gz', 'chip_io.gds.gz')
if ($Caravel) { $skyFiles += 'caravel.gds.gz' }
$sky = Get-Designs 'https://raw.githubusercontent.com/efabless/caravel/main/gds' $skyDir $skyFiles
Build-Project (Join-Path $Dest 'sky130') (Join-Path $skyDir 'sky130.lyp') (Join-Path $skyDir 'sky130.lyt') $sky

Write-Host '== GF180MCU (wafer.space)'
$gf = 'https://raw.githubusercontent.com/wafer-space/gf180mcu/main/gf180mcuD'
$gfDir = Join-Path $downloads 'gf180'
Get-Download "$gf/libs.tech/klayout/tech/gf180mcu.lyp" (Join-Path $gfDir 'gf180mcu.lyp')
Get-Download "$gf/libs.tech/klayout/tech/gf180mcu.lyt" (Join-Path $gfDir 'gf180mcu.lyt')
$gfLibs = [ordered]@{}
foreach ($d in @(
        @('gf180mcu_fd_sc_mcu7t5v0', 'gf180mcu_fd_sc_mcu7t5v0/gds/gf180mcu_fd_sc_mcu7t5v0.gds'),
        @('gf180mcu_fd_ip_sram', 'gf180mcu_fd_ip_sram/gds/gf180mcu_fd_ip_sram__sram512x8m8wm1.gds'))) {
    $out = Join-Path $gfDir (Split-Path $d[1] -Leaf)
    Get-Download "$gf/libs.ref/$($d[1])" $out
    $gfLibs[$d[0]] = $out
}
Build-Project (Join-Path $Dest 'gf180') (Join-Path $gfDir 'gf180mcu.lyp') (Join-Path $gfDir 'gf180mcu.lyt') $gfLibs

Write-Host ''
Write-Host 'Done. Open a project with:'
Write-Host "  cargo run --release -- $(Join-Path $Dest 'sky130')"
Write-Host "  cargo run --release -- $(Join-Path $Dest 'gf180')"
