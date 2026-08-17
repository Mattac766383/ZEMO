# RepoRoot resolution and packaging smoke for package-windows-beta.ps1.
# Does not weaken installer presence or repository-marker checks.

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($PSScriptRoot)) {
    throw "package-windows-beta.test.ps1 requires a script path."
}

$packageScript = Join-Path $PSScriptRoot "package-windows-beta.ps1"
if (-not (Test-Path -LiteralPath $packageScript)) {
    throw "package-windows-beta.ps1 was not found beside the test."
}

$source = Get-Content -LiteralPath $packageScript -Raw
if ($source -match '(?s)param\s*\([^)]*Join-Path\s+\$PSScriptRoot') {
    throw "package-windows-beta.ps1 must not resolve RepoRoot inside param()."
}
if ($source -notmatch '\$env:GITHUB_WORKSPACE') {
    throw "package-windows-beta.ps1 must fall back to GITHUB_WORKSPACE."
}
foreach ($marker in @("Cargo.toml", "package.json", "apps/desktop/src-tauri")) {
    if ($source -notmatch [regex]::Escape($marker)) {
        throw "package-windows-beta.ps1 must validate repository marker $marker."
    }
}

function Resolve-ZemoWindowsRepoRootForTest {
    param(
        [string]$RepoRoot,
        [string]$ScriptRoot,
        [string]$Workspace,
        [string]$Cwd
    )
    if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
        if (-not [string]::IsNullOrWhiteSpace($ScriptRoot)) {
            $RepoRoot = (Resolve-Path (Join-Path $ScriptRoot "../..")).Path
        }
        elseif (-not [string]::IsNullOrWhiteSpace($Workspace)) {
            $RepoRoot = (Resolve-Path $Workspace).Path
        }
        else {
            $RepoRoot = $Cwd
        }
    }
    return $RepoRoot
}

$workspaceFromScript = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$fallbackFromScript = Resolve-ZemoWindowsRepoRootForTest -RepoRoot "" -ScriptRoot $PSScriptRoot -Workspace "" -Cwd "unused"
if ($fallbackFromScript -ne $workspaceFromScript) {
    throw "PSScriptRoot fallback resolved '$fallbackFromScript' instead of '$workspaceFromScript'."
}

$tempWorkspace = Join-Path ([System.IO.Path]::GetTempPath()) ("zemo-package-root-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tempWorkspace | Out-Null
try {
    $resolvedWorkspace = (Resolve-Path -LiteralPath $tempWorkspace).Path
    $fromWorkspace = Resolve-ZemoWindowsRepoRootForTest -RepoRoot "" -ScriptRoot "" -Workspace $resolvedWorkspace -Cwd "unused"
    if ($fromWorkspace -ne $resolvedWorkspace) {
        throw "GITHUB_WORKSPACE fallback resolved '$fromWorkspace' instead of '$resolvedWorkspace'."
    }
    $fromCwd = Resolve-ZemoWindowsRepoRootForTest -RepoRoot "" -ScriptRoot "" -Workspace "" -Cwd $resolvedWorkspace
    if ($fromCwd -ne $resolvedWorkspace) {
        throw "Get-Location fallback resolved '$fromCwd' instead of '$resolvedWorkspace'."
    }
    $explicit = Resolve-ZemoWindowsRepoRootForTest -RepoRoot $resolvedWorkspace -ScriptRoot "ignored" -Workspace "ignored" -Cwd "ignored"
    if ($explicit -ne $resolvedWorkspace) {
        throw "Explicit RepoRoot was not preserved."
    }
} finally {
    Remove-Item -LiteralPath $tempWorkspace -Recurse -Force -ErrorAction SilentlyContinue
}

function New-FakeRepo {
    param([string]$Root, [switch]$WithInstaller, [switch]$WithMarkers)
    New-Item -ItemType Directory -Force -Path $Root | Out-Null
    if ($WithMarkers) {
        Set-Content -LiteralPath (Join-Path $Root "Cargo.toml") -Value "[workspace]" -Encoding ascii
        Set-Content -LiteralPath (Join-Path $Root "package.json") -Value "{}" -Encoding ascii
        New-Item -ItemType Directory -Force -Path (Join-Path $Root "apps/desktop/src-tauri") | Out-Null
    }
    if ($WithInstaller) {
        $nsisDir = Join-Path $Root "target/release/bundle/nsis"
        New-Item -ItemType Directory -Force -Path $nsisDir | Out-Null
        [System.IO.File]::WriteAllBytes((Join-Path $nsisDir "ZEMO_0.1.0_x64-setup.exe"), [byte[]](0x4D, 0x5A, 0x00, 0x01))
        $qualDir = Join-Path $Root "target/windows-qualification"
        New-Item -ItemType Directory -Force -Path $qualDir | Out-Null
        Set-Content -LiteralPath (Join-Path $qualDir "apply-decision.json") -Value '{"qualification_status":"PASS"}' -Encoding ascii
        Set-Content -LiteralPath (Join-Path $qualDir "qualification-summary.txt") -Value "Apply qualified: YES" -Encoding ascii
    }
}

function Invoke-PackageScript {
    param(
        [string]$RepoRoot,
        [string]$DistributionKind = "apply"
    )
    $scriptArgs = @(
        "-NoProfile",
        "-ExecutionPolicy", "Bypass",
        "-File", $packageScript,
        "-DistributionKind", $DistributionKind,
        "-DistTag", "0.1.0-beta.6",
        "-GitCommit", "test-commit"
    )
    if ($PSBoundParameters.ContainsKey("RepoRoot")) {
        $scriptArgs += @("-RepoRoot", $RepoRoot)
    }
    $output = & powershell @scriptArgs 2>&1
    return @{
        ExitCode = $LASTEXITCODE
        Output = ($output | Out-String)
    }
}

$missingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("zemo-package-missing-" + [guid]::NewGuid().ToString("N"))
$missing = Invoke-PackageScript -RepoRoot $missingRoot
if ($missing.ExitCode -eq 0) {
    throw "Empty/missing RepoRoot must fail. Output:`n$($missing.Output)"
}
if ($missing.Output -notmatch "does not exist") {
    throw "Missing RepoRoot must report that the path does not exist. Output:`n$($missing.Output)"
}

$noMarkers = Join-Path ([System.IO.Path]::GetTempPath()) ("zemo-package-nomarkers-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $noMarkers | Out-Null
try {
    $markerFailure = Invoke-PackageScript -RepoRoot $noMarkers
    if ($markerFailure.ExitCode -eq 0) {
        throw "RepoRoot without markers must fail."
    }
    if ($markerFailure.Output -notmatch "missing Cargo.toml") {
        throw "Missing markers must fail clearly. Output:`n$($markerFailure.Output)"
    }
} finally {
    Remove-Item -LiteralPath $noMarkers -Recurse -Force -ErrorAction SilentlyContinue
}

$noInstaller = Join-Path ([System.IO.Path]::GetTempPath()) ("zemo-package-noinstaller-" + [guid]::NewGuid().ToString("N"))
New-FakeRepo -Root $noInstaller -WithMarkers
try {
    $installerFailure = Invoke-PackageScript -RepoRoot $noInstaller
    if ($installerFailure.ExitCode -eq 0) {
        throw "RepoRoot without an NSIS installer must fail."
    }
    if ($installerFailure.Output -notmatch "No NSIS installer") {
        throw "Missing installer safety check was weakened. Output:`n$($installerFailure.Output)"
    }
} finally {
    Remove-Item -LiteralPath $noInstaller -Recurse -Force -ErrorAction SilentlyContinue
}

$fakeRepo = Join-Path ([System.IO.Path]::GetTempPath()) ("zemo-package-ok-" + [guid]::NewGuid().ToString("N"))
New-FakeRepo -Root $fakeRepo -WithMarkers -WithInstaller
try {
    $ok = Invoke-PackageScript -RepoRoot $fakeRepo
    if ($ok.ExitCode -ne 0) {
        throw "Explicit RepoRoot packaging failed:`n$($ok.Output)"
    }
    $dist = Join-Path $fakeRepo "target/windows-qualification/dist"
    $expected = @(
        "ZEMO-0.1.0-beta.6-windows-x64.exe",
        "SHA256SUMS.txt",
        "BUILDINFO.txt",
        "README-FIRST.txt",
        "qualification-summary.txt"
    )
    foreach ($name in $expected) {
        $path = Join-Path $dist $name
        if (-not (Test-Path -LiteralPath $path)) {
            throw "Packaging did not write $name under the explicit RepoRoot."
        }
    }
    $buildInfo = Get-Content -LiteralPath (Join-Path $dist "BUILDINFO.txt") -Raw
    if ($buildInfo -notmatch "test-commit") {
        throw "BUILDINFO.txt did not record the explicit GitCommit."
    }
    if ($ok.Output -notmatch [regex]::Escape((Resolve-Path -LiteralPath $fakeRepo).Path)) {
        throw "Packaging did not echo the explicit RepoRoot. Output:`n$($ok.Output)"
    }
} finally {
    Remove-Item -LiteralPath $fakeRepo -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "package-windows-beta.ps1 RepoRoot resolution: PASS"
Write-Host "package-windows-beta.ps1 marker and installer safety: PASS"
Write-Host "package-windows-beta.ps1 explicit RepoRoot packaging: PASS"
