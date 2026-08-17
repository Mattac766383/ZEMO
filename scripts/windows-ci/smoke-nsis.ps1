# Silent NSIS install / file check / uninstall when the runner allows it.
# Does not claim human GUI qualification. Must not delete user documents.

param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path,
    [string]$InstallerPath
)

$ErrorActionPreference = "Stop"
Set-Location $RepoRoot

if (-not $InstallerPath) {
    $dist = Join-Path $RepoRoot "target/windows-qualification/dist"
    $InstallerPath = Get-ChildItem -LiteralPath $dist -Filter "ZEMO-*-windows-x64.exe" |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1 -ExpandProperty FullName
}
if (-not $InstallerPath -or -not (Test-Path -LiteralPath $InstallerPath)) {
    throw "Installer not found for NSIS smoke."
}

$installDir = Join-Path $env:RUNNER_TEMP "zemo-nsis-smoke"
if (-not $env:RUNNER_TEMP) {
    $installDir = Join-Path $env:TEMP "zemo-nsis-smoke"
}
if (Test-Path -LiteralPath $installDir) {
    Remove-Item -LiteralPath $installDir -Recurse -Force
}
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

$notes = @()
$installStatus = "NOT RUN"
try {
    $args = @("/S", "/D=$installDir")
    $p = Start-Process -FilePath $InstallerPath -ArgumentList $args -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        throw "NSIS silent install exited $($p.ExitCode)"
    }
    $app = Get-ChildItem -LiteralPath $installDir -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -in @("ZEMO.exe", "zemo.exe", "desktop.exe") } |
        Select-Object -First 1
    $sidecar = Get-ChildItem -LiteralPath $installDir -Recurse -Filter "operation-executor*.exe" -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if (-not $app) { throw "Installed tree is missing ZEMO.exe (or desktop.exe cargo binary)" }
    if (-not $sidecar) { throw "Installed tree is missing operation-executor sidecar" }
    $installStatus = "PASS"
    $notes += "Silent install placed ZEMO.exe and the sidecar under $installDir"

    try {
        $proc = Start-Process -FilePath $app.FullName -PassThru -WindowStyle Hidden
        Start-Sleep -Seconds 4
        if ($proc -and -not $proc.HasExited) {
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            $notes += "Installed process started. GUI interaction NOT TESTED."
        } else {
            $notes += "Installed process exited $($proc.ExitCode). GUI interaction NOT TESTED."
        }
    } catch {
        $notes += "Installed launch could not be automated: $($_.Exception.Message). GUI NOT TESTED."
    }
} catch {
    $installStatus = "PARTIAL"
    $notes += "Silent install/smoke not completed: $($_.Exception.Message)"
}

$uninstallStatus = "NOT RUN"
$uninstaller = Get-ChildItem -LiteralPath $installDir -Recurse -Filter "uninstall.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($installStatus -eq "PASS" -and $uninstaller) {
    try {
        $u = Start-Process -FilePath $uninstaller.FullName -ArgumentList "/S" -Wait -PassThru
        Start-Sleep -Seconds 2
        $uninstallStatus = if ($u.ExitCode -eq 0) { "PASS" } else { "PARTIAL" }
        $notes += "Uninstall exit $($u.ExitCode). Application files should be removed; user documents are not targeted."
    } catch {
        $uninstallStatus = "PARTIAL"
        $notes += "Uninstall could not be automated: $($_.Exception.Message)"
    }
} elseif ($installStatus -eq "PASS") {
    $uninstallStatus = "NOT RUN"
    $notes += "No uninstall.exe found after silent install."
}

$result = @{
    install_status = $installStatus
    uninstall_status = $uninstallStatus
    notes = ($notes -join " ")
}
$resultPath = Join-Path $RepoRoot "target/windows-qualification/nsis-smoke.json"
($result | ConvertTo-Json -Depth 4) | Set-Content -LiteralPath $resultPath -Encoding utf8
Write-Host "NSIS smoke install: $installStatus"
Write-Host "NSIS smoke uninstall: $uninstallStatus"
Write-Host ($notes -join "`n")

if ($env:GITHUB_OUTPUT) {
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "install=$installStatus"
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "uninstall=$uninstallStatus"
}

# A failed silent install on a headless runner is not a product safety failure.
if ($installStatus -eq "FAIL") {
    exit 1
}
exit 0
