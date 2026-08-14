# M15-A Windows qualification entrypoint (PowerShell).
# Run from a real Windows host/runner with Rust MSVC + Node 22+.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File .\scripts\windows-qualification.ps1
#   powershell -ExecutionPolicy Bypass -File .\scripts\windows-qualification.ps1 -PrepOnly
#   powershell -ExecutionPolicy Bypass -File .\scripts\windows-qualification.ps1 -ModelDir D:\models\granite

param(
    [switch]$PrepOnly,
    [switch]$SkipCargoCheck,
    [string]$ModelDir = $env:SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

if ($ModelDir) {
    $env:SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR = $ModelDir
}

$args = @()
if ($PrepOnly) { $args += "--prep-only" }
if ($SkipCargoCheck) { $args += "--skip-cargo-check" }
$args += "--json"

Write-Host "M15-A Windows qualification starting in $RepoRoot"
node .\scripts\windows-qualification\run.mjs @args
exit $LASTEXITCODE
