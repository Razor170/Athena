# Downloads DOTLAN EveMaps' region maps for the Intel map's layout.
#
#   ./xtask/dotlan.ps1 -Sde sde.zip [-Folder dotlan] [-Refresh]
#
# `cargo xtask regions` lists the file of every region the Intel map shows
# (taken from the SDE, so a new region is picked up on its own). Each file
# that is missing from the folder is downloaded from
# https://evemaps.dotlan.net/svg/<Region>.svg, one after another about a
# second apart. With -Refresh every file is downloaded again, and the old
# copy is kept when a download fails.
#
# The maps are (c) Wollari, Daniel Hoffend. They are never committed: CI
# downloads them and `cargo xtask sde --dotlan <folder>` turns them into
# positions inside static.sqlite (see PLAN.md, Step 6).
#
# A failed download never fails the script: the region then keeps its cached
# file, or without one falls back to CCP's position2D layout in the xtask.

param(
    [Parameter(Mandatory = $true)][string]$Sde,
    [string]$Folder = "dotlan",
    [switch]$Refresh
)

$ErrorActionPreference = "Stop"

$files = @(cargo run --quiet --locked --package xtask -- regions $Sde)
if ($LASTEXITCODE -ne 0 -or $files.Count -eq 0) {
    Write-Error "cargo xtask regions failed"
    exit 1
}

New-Item -ItemType Directory -Force -Path $Folder | Out-Null
$userAgent = "Athena (github.com/Razor170/Athena)"
$fetched = 0
$failed = 0

foreach ($file in $files) {
    $file = $file.Trim()
    if ($file -eq "") { continue }
    $path = Join-Path $Folder $file
    if ((Test-Path $path) -and -not $Refresh) { continue }

    # Be polite to Dotlan: one request at a time, about a second apart.
    if ($fetched + $failed -gt 0) { Start-Sleep -Seconds 1 }

    # Download next to the old file first, so a failed or half download
    # never replaces a good cached copy.
    $tmp = "$path.download"
    try {
        Invoke-WebRequest -UseBasicParsing -TimeoutSec 60 -UserAgent $userAgent `
            -OutFile $tmp "https://evemaps.dotlan.net/svg/$file"
        Move-Item -Force $tmp $path
        $fetched++
    } catch {
        Remove-Item -Force -ErrorAction SilentlyContinue $tmp
        if (Test-Path $path) {
            Write-Warning "Can't download $file, keeping the cached copy: $_"
        } else {
            Write-Warning "Can't download $file, that region uses position2D: $_"
        }
        $failed++
    }
}

$have = @(Get-ChildItem -Path $Folder -Filter *.svg).Count
Write-Output "Dotlan maps: $fetched downloaded, $failed failed, $have of $($files.Count) in $Folder"
exit 0
