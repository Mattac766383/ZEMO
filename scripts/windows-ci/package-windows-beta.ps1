# Collect the NSIS installer and write private-beta distributable metadata.
# Does not upload anything. Does not create a GitHub Release.

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("apply", "propose-only")]
    [string]$DistributionKind,

    [string]$RepoRoot,
    [string]$DistTag = "0.1.0-beta.6",
    [string]$GitCommit = $env:GITHUB_SHA
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    if (-not [string]::IsNullOrWhiteSpace($PSScriptRoot)) {
        $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
    }
    elseif (-not [string]::IsNullOrWhiteSpace($env:GITHUB_WORKSPACE)) {
        $RepoRoot = (Resolve-Path $env:GITHUB_WORKSPACE).Path
    }
    else {
        $RepoRoot = (Get-Location).Path
    }
}

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    throw "Repository root resolved to an empty path."
}
if (-not (Test-Path -LiteralPath $RepoRoot)) {
    throw "Repository root does not exist: $RepoRoot"
}
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path

foreach ($marker in @("Cargo.toml", "package.json", "apps/desktop/src-tauri")) {
    $markerPath = Join-Path $RepoRoot $marker
    if (-not (Test-Path -LiteralPath $markerPath)) {
        throw "Repository root is missing $marker : $RepoRoot"
    }
}

function Assert-NonEmptyPath {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [string]$Value
    )
    if ([string]::IsNullOrWhiteSpace($Value)) {
        throw "$Name must not be an empty path."
    }
}

Assert-NonEmptyPath -Name "RepoRoot" -Value $RepoRoot
Set-Location -LiteralPath $RepoRoot

$qualificationRoot = Join-Path $RepoRoot "target/windows-qualification"
Assert-NonEmptyPath -Name "qualificationRoot" -Value $qualificationRoot
$outDir = Join-Path $qualificationRoot "dist"
Assert-NonEmptyPath -Name "artifact root" -Value $outDir
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$searchRoots = @(
    (Join-Path $RepoRoot "target/release/bundle/nsis"),
    (Join-Path $RepoRoot "target/x86_64-pc-windows-msvc/release/bundle/nsis"),
    (Join-Path $RepoRoot "apps/desktop/src-tauri/target/release/bundle/nsis"),
    (Join-Path $RepoRoot "apps/desktop/src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis")
)
foreach ($root in $searchRoots) {
    Assert-NonEmptyPath -Name "bundle root" -Value $root
}

$found = @()
foreach ($root in $searchRoots) {
    if (Test-Path -LiteralPath $root) {
        $found += Get-ChildItem -LiteralPath $root -Filter "*.exe" -File -ErrorAction SilentlyContinue
    }
}
$found = $found | Sort-Object LastWriteTime -Descending
if (-not $found -or $found.Count -eq 0) {
    throw "No NSIS installer (.exe) found under bundle/nsis. Tauri build did not produce an installer."
}

$source = $found[0]
Assert-NonEmptyPath -Name "installer source" -Value $source.FullName
$installerName = if ($DistributionKind -eq "apply") {
    "ZEMO-$DistTag-windows-x64.exe"
} else {
    "ZEMO-$DistTag-windows-x64-propose-only.exe"
}
$installerPath = Join-Path $outDir $installerName
Assert-NonEmptyPath -Name "installer destination" -Value $installerPath
Copy-Item -LiteralPath $source.FullName -Destination $installerPath -Force

$hash = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
$sumsPath = Join-Path $outDir "SHA256SUMS.txt"
Assert-NonEmptyPath -Name "SHA output" -Value $sumsPath
"${hash}  ${installerName}" | Set-Content -LiteralPath $sumsPath -Encoding ascii

$osName = [System.Environment]::OSVersion.VersionString
$osVersion = [System.Environment]::OSVersion.VersionString
if (Get-Command Get-CimInstance -ErrorAction SilentlyContinue) {
    try {
        $caption = (Get-CimInstance Win32_OperatingSystem -ErrorAction Stop | Select-Object -First 1).Caption
        if (-not [string]::IsNullOrWhiteSpace($caption)) {
            $osName = $caption
        }
    } catch {
        $osName = $osVersion
    }
}
$arch = $env:PROCESSOR_ARCHITECTURE
if ([string]::IsNullOrWhiteSpace($arch)) {
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
}
$applyQualified = $DistributionKind -eq "apply"
$decisionPath = Join-Path $qualificationRoot "apply-decision.json"
Assert-NonEmptyPath -Name "apply-decision path" -Value $decisionPath
$qualificationStatus = "UNKNOWN"
if (Test-Path -LiteralPath $decisionPath) {
    $decision = Get-Content -LiteralPath $decisionPath -Raw | ConvertFrom-Json
    $qualificationStatus = $decision.qualification_status
}

$signing = "NOT CONFIGURED"
if ($env:WINDOWS_CERTIFICATE -or $env:TAURI_SIGNING_PRIVATE_KEY) {
    $signing = "SECRET PRESENT (not printed; signing still not claimed configured unless the build used it)"
}

$buildInfo = @"
ZEMO
version: 0.1.0
distribution tag: $DistTag
distribution kind: $DistributionKind
git commit: $GitCommit
Windows runner: $osName
Windows version: $osVersion
architecture: $arch
target: x86_64-pc-windows-msvc
qualification status: $qualificationStatus
Apply qualification status: $(if ($applyQualified) { "PASS" } else { "NOT QUALIFIED / PROPOSE-ONLY" })
WINDOWS SIGNING: $signing
SMARTSCREEN USER EXPERIENCE: NOT QUALIFIED
GUI interaction: NOT TESTED
build timestamp: $((Get-Date).ToUniversalTime().ToString("o"))
source installer: $($source.Name)
"@
$buildInfoPath = Join-Path $outDir "BUILDINFO.txt"
Assert-NonEmptyPath -Name "BUILDINFO output" -Value $buildInfoPath
Set-Content -LiteralPath $buildInfoPath -Value $buildInfo.Trim() -Encoding utf8

if ($applyQualified) {
    $readme = @"
ZEMO — Bêta privée Windows
==========================

Cette version a passé la qualification native NTFS sur le runner GitHub Windows.
Après votre confirmation explicite, ZEMO peut déplacer et renommer des fichiers
du plan approuvé. Vous pouvez ensuite annuler lorsque c’est encore possible.

Installation
------------
1. Téléchargez l’installateur $installerName.
2. Lancez l’installateur.
3. Windows peut afficher SmartScreen, car cette bêta privée n’est pas signée.
   Choisissez « Informations complémentaires » puis « Exécuter quand même »
   uniquement si vous avez reçu ce fichier du mainteneur ZEMO.
4. Ne désactivez pas Windows Defender.
5. Utilisez d’abord un dossier de test, avec des copies de fichiers.

Ne pas faire
------------
- Ne pas organiser Bureau / Documents / Téléchargements personnels au premier essai.
- Ne pas désactiver les protections Windows.
- Ne pas s’attendre à une réputation SmartScreen : NON QUALIFIÉE.
- Cette version n’est pas une publication publique et n’a pas de mise à jour automatique.

Identifiant technique : com.workingname.organizer
Version : 0.1.0 ($DistTag)
Fichier : $installerName
"@
} else {
    $readme = @"
ZEMO — Bêta privée Windows (proposition uniquement)
===================================================

Cette version NE DÉPLACE PAS les fichiers.
ZEMO peut analyser, proposer une organisation, rechercher et surveiller.
Le bouton d’application réelle n’est pas activé.

Installation
------------
1. Téléchargez l’installateur $installerName.
2. Lancez l’installateur.
3. Windows peut afficher SmartScreen, car cette bêta privée n’est pas signée.
   Choisissez « Informations complémentaires » puis « Exécuter quand même »
   uniquement si vous avez reçu ce fichier du mainteneur ZEMO.
4. Ne désactivez pas Windows Defender.
5. Utilisez d’abord un dossier de test, avec des copies de fichiers.

Les fichiers ne sont PAS déplacés par cette version.

Identifiant technique : com.workingname.organizer
Version : 0.1.0 ($DistTag)
Fichier : $installerName
"@
}

$readmePath = Join-Path $outDir "README-FIRST.txt"
Assert-NonEmptyPath -Name "README output" -Value $readmePath
Set-Content -LiteralPath $readmePath -Value $readme.Trim() -Encoding utf8

$summarySource = Join-Path $qualificationRoot "qualification-summary.txt"
$summaryDest = Join-Path $outDir "qualification-summary.txt"
Assert-NonEmptyPath -Name "qualification-summary source" -Value $summarySource
Assert-NonEmptyPath -Name "qualification-summary path" -Value $summaryDest
if (Test-Path -LiteralPath $summarySource) {
    Copy-Item -LiteralPath $summarySource -Destination $summaryDest -Force
} else {
    Set-Content -LiteralPath $summaryDest -Value "qualification-summary missing" -Encoding utf8
}

$package = @{
    installer_status = "PASS"
    artifact_status = "PASS"
    installer_name = $installerName
    installer_path = $installerPath
    sha256 = $hash
    distribution_kind = $DistributionKind
    repo_root = $RepoRoot
    notes = "NSIS installer packaged. GUI interaction NOT TESTED. SmartScreen NOT QUALIFIED."
}
$packagePath = Join-Path $qualificationRoot "package-result.json"
Assert-NonEmptyPath -Name "package-result path" -Value $packagePath
($package | ConvertTo-Json -Depth 4) | Set-Content -LiteralPath $packagePath -Encoding utf8

if ($env:GITHUB_OUTPUT) {
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "installer_name=$installerName"
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "dist_dir=$outDir"
}

Write-Host "RepoRoot $RepoRoot"
Write-Host "Packaged $installerName"
Write-Host "SHA-256 $hash"
Write-Host "Dist $outDir"
