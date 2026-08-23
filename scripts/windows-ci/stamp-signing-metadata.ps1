param(
    [string]$RepoRoot
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    if (-not [string]::IsNullOrWhiteSpace($env:GITHUB_WORKSPACE)) {
        $RepoRoot = $env:GITHUB_WORKSPACE
    } elseif (-not [string]::IsNullOrWhiteSpace($PSScriptRoot)) {
        $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
    } else {
        $RepoRoot = (Get-Location).Path
    }
}
$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
$qualificationRoot = Join-Path $RepoRoot "target/windows-qualification"
$distRoot = Join-Path $qualificationRoot "dist"
$authenticodePath = Join-Path $qualificationRoot "authenticode-report.json"

if (-not (Test-Path -LiteralPath $authenticodePath)) {
    throw "Authenticode report is missing: $authenticodePath"
}
$auth = Get-Content -LiteralPath $authenticodePath -Raw | ConvertFrom-Json
$valid = [bool]$auth.all_required_valid
$signingEnabled = [string]$env:ZEMO_WINDOWS_SIGNING_ENABLED -eq "true"

$manifestPath = Join-Path $distRoot "beta-manifest.json"
if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "Beta manifest is missing: $manifestPath"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$manifest.signing.configured = $valid
$manifest.signing.secret_present = $signingEnabled
$manifest.signing | Add-Member -NotePropertyName authenticode_valid -NotePropertyValue $valid -Force
$manifest.signing | Add-Member -NotePropertyName certificate_thumbprint -NotePropertyValue ([string]$env:ZEMO_WINDOWS_CERT_THUMBPRINT) -Force
$manifest.signing.smartscreen_external_user_experience_qualified = $false
($manifest | ConvertTo-Json -Depth 10) | Set-Content -LiteralPath $manifestPath -Encoding utf8

$buildInfoPath = Join-Path $distRoot "BUILDINFO.txt"
if (Test-Path -LiteralPath $buildInfoPath) {
    $buildInfo = Get-Content -LiteralPath $buildInfoPath -Raw
    $replacement = if ($valid) { "WINDOWS SIGNING: AUTHENTICODE VALID" } else { "WINDOWS SIGNING: NOT CONFIGURED" }
    $buildInfo = [regex]::Replace($buildInfo, "WINDOWS SIGNING:.*", $replacement)
    Set-Content -LiteralPath $buildInfoPath -Value $buildInfo.TrimEnd() -Encoding utf8
}

$readmePath = Join-Path $distRoot "README-FIRST.txt"
if ($valid -and (Test-Path -LiteralPath $readmePath)) {
    $readme = Get-Content -LiteralPath $readmePath -Raw
    $unsignedBlock = "Windows peut afficher SmartScreen, car cette bêta privée n’est pas signée.`r?`n\s*Choisissez « Informations complémentaires » puis « Exécuter quand même »`r?`n\s*uniquement si vous avez reçu ce fichier du mainteneur ZEMO\."
    $signedText = "La signature Authenticode de cette bêta est valide. SmartScreen peut néanmoins afficher un avertissement tant que la réputation du certificat ou de l’application n’est pas établie. N’exécutez le fichier que s’il provient du mainteneur ZEMO."
    $readme = [regex]::Replace($readme, $unsignedBlock, $signedText)
    Set-Content -LiteralPath $readmePath -Value $readme.TrimEnd() -Encoding utf8
}

Copy-Item -LiteralPath $authenticodePath -Destination (Join-Path $distRoot "authenticode-report.json") -Force
Write-Host "Signing metadata stamped. Authenticode valid: $valid"
Write-Host "SmartScreen reputation remains explicitly NOT QUALIFIED."
