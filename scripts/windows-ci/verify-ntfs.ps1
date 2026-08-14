# Fail-closed NTFS gate for Windows qualification.
# Creates a disposable sandbox under the runner temp volume and refuses
# mutation qualification when that volume is not NTFS.

$ErrorActionPreference = "Stop"

$base = $env:RUNNER_TEMP
if (-not $base) {
    $base = $env:TEMP
}
if (-not $base) {
    Write-Error "No RUNNER_TEMP or TEMP directory is available."
    exit 1
}

$root = Join-Path $base "zemo-windows-qualification"
New-Item -ItemType Directory -Force -Path $root | Out-Null

$item = Get-Item -LiteralPath $root
$filesystem = [string]$item.PSDrive.FileSystem
$rootFull = $item.FullName

Write-Host "Qualification sandbox: $rootFull"
Write-Host "Filesystem: $filesystem"

if ($filesystem -ne "NTFS") {
    Write-Error "Qualification volume is '$filesystem', not NTFS. Apply qualification must not run."
    exit 1
}

if ($env:GITHUB_ENV) {
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "TEMP=$rootFull"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "TMP=$rootFull"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "TMPDIR=$rootFull"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "ZEMO_WINDOWS_QUALIFICATION_ROOT=$rootFull"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "ZEMO_WINDOWS_QUALIFICATION_FILESYSTEM=$filesystem"
}

if ($env:GITHUB_OUTPUT) {
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "filesystem=$filesystem"
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "sandbox=$rootFull"
}

Write-Host "NTFS gate: PASS"
exit 0
