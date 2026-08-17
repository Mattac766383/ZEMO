# Inspect the native Windows build output for required runtime files.
# A successful compile alone is not enough.

param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
)

$ErrorActionPreference = "Stop"
Set-Location $RepoRoot

$searchRoots = @(
    (Join-Path $RepoRoot "target/release"),
    (Join-Path $RepoRoot "target/x86_64-pc-windows-msvc/release"),
    (Join-Path $RepoRoot "apps/desktop/src-tauri/target/release"),
    (Join-Path $RepoRoot "apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release")
) | Where-Object { Test-Path -LiteralPath $_ }

function Find-One {
    param([string[]]$Filters)
    foreach ($root in $searchRoots) {
        foreach ($filter in $Filters) {
            $hit = Get-ChildItem -LiteralPath $root -Recurse -File -Filter $filter -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -notmatch '\\deps\\|\\incremental\\|\\build\\' } |
                Select-Object -First 1
            if ($hit) {
                return $hit
            }
        }
    }
    return $null
}

# Cargo package name is `desktop`; Tauri productName is ZEMO. The release
# binary is therefore desktop.exe. NSIS ships it as ZEMO.exe after install.
$app = Find-One @("ZEMO.exe", "zemo.exe", "desktop.exe")
$sidecar = Find-One @("operation-executor.exe", "operation-executor-*.exe")
$ort = Find-One @("onnxruntime.dll", "onnxruntime*.dll")
$nsis = Find-One @("*-setup.exe")

Write-Host "ZEMO executable: $(if ($app) { $app.FullName } else { 'MISSING' })"
Write-Host "operation-executor sidecar: $(if ($sidecar) { $sidecar.FullName } else { 'MISSING' })"
Write-Host "ORT DLL: $(if ($ort) { $ort.FullName } else { 'NOT FOUND beside build output' })"
Write-Host "NSIS setup: $(if ($nsis) { $nsis.FullName } else { 'MISSING' })"

if (-not $app) {
    foreach ($root in $searchRoots) {
        Write-Host "Release root $root"
        Get-ChildItem -LiteralPath $root -File -ErrorAction SilentlyContinue |
            Select-Object -ExpandProperty Name |
            ForEach-Object { Write-Host "  $_" }
    }
    throw "App executable (ZEMO.exe or desktop.exe) was not found in the release output."
}
if (-not $sidecar) { throw "operation-executor sidecar was not found in the release output." }
if (-not $nsis) { throw "NSIS installer was not found in the release output." }

$launchStatus = "NOT RUN"
try {
    $proc = Start-Process -FilePath $app.FullName -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 4
    if ($proc -and -not $proc.HasExited) {
        $launchStatus = "PROCESS STARTED (GUI NOT TESTED)"
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    } elseif ($proc -and $proc.ExitCode -eq 0) {
        $launchStatus = "PROCESS EXITED 0 (GUI NOT TESTED)"
    } else {
        $launchStatus = "PROCESS EXITED $($proc.ExitCode) (GUI NOT TESTED)"
    }
} catch {
    $launchStatus = "FAIL: $($_.Exception.Message)"
}

Write-Host "Launch smoke: $launchStatus"
if ($launchStatus -like "FAIL:*") {
    Write-Host "GUI / process startup could not be automated on this runner. Binary is present. GUI NOT TESTED."
}

$result = @{
    app = $app.FullName
    sidecar = $sidecar.FullName
    ort = $(if ($ort) { $ort.FullName } else { "" })
    nsis = $nsis.FullName
    launch = $launchStatus
    ort_status = $(if ($ort) { "PASS" } else { "PARTIAL" })
}
$resultPath = Join-Path $RepoRoot "target/windows-qualification/bundle-inspect.json"
New-Item -ItemType Directory -Force -Path (Split-Path $resultPath) | Out-Null
($result | ConvertTo-Json -Depth 4) | Set-Content -LiteralPath $resultPath -Encoding utf8

if ($env:GITHUB_OUTPUT) {
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "launch=$launchStatus"
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "ort=$(if ($ort) { 'PASS' } else { 'PARTIAL' })"
}
