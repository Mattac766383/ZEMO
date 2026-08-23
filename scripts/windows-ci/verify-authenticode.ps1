param(
    [string]$RepoRoot,
    [switch]$RequireSigned
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
$inspectPath = Join-Path $qualificationRoot "bundle-inspect.json"
if (-not (Test-Path -LiteralPath $inspectPath)) {
    throw "Bundle inspection evidence is missing: $inspectPath"
}

$bundle = Get-Content -LiteralPath $inspectPath -Raw | ConvertFrom-Json
$targets = [ordered]@{
    app = [string]$bundle.app
    installer = [string]$bundle.nsis
}
if (-not [string]::IsNullOrWhiteSpace([string]$bundle.sidecar)) {
    $targets.sidecar = [string]$bundle.sidecar
}

$report = [ordered]@{
    required = [bool]$RequireSigned
    all_required_valid = $true
    files = [ordered]@{}
}

foreach ($entry in $targets.GetEnumerator()) {
    $name = $entry.Key
    $path = $entry.Value
    if ([string]::IsNullOrWhiteSpace($path) -or -not (Test-Path -LiteralPath $path)) {
        if ($name -in @("app", "installer")) {
            $report.all_required_valid = $false
        }
        $report.files[$name] = [ordered]@{
            path = $path
            status = "MISSING"
            subject = ""
            thumbprint = ""
            timestamped = $false
        }
        continue
    }

    $signature = Get-AuthenticodeSignature -FilePath $path
    $valid = $signature.Status -eq [System.Management.Automation.SignatureStatus]::Valid
    if ($name -in @("app", "installer") -and -not $valid) {
        $report.all_required_valid = $false
    }
    $report.files[$name] = [ordered]@{
        path = $path
        status = [string]$signature.Status
        subject = if ($signature.SignerCertificate) { $signature.SignerCertificate.Subject } else { "" }
        thumbprint = if ($signature.SignerCertificate) { $signature.SignerCertificate.Thumbprint } else { "" }
        timestamped = $null -ne $signature.TimeStamperCertificate
    }
    Write-Host "$name Authenticode: $($signature.Status)"
}

New-Item -ItemType Directory -Force -Path $qualificationRoot | Out-Null
$reportPath = Join-Path $qualificationRoot "authenticode-report.json"
($report | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $reportPath -Encoding utf8

if ($report.all_required_valid) {
    Write-Host "AUTHENTICODE: required application and installer signatures are valid."
    if ($env:GITHUB_ENV) {
        Add-Content -LiteralPath $env:GITHUB_ENV -Value "ZEMO_WINDOWS_AUTHENTICODE_STATUS=VALID"
    }
} else {
    if ($env:GITHUB_ENV) {
        Add-Content -LiteralPath $env:GITHUB_ENV -Value "ZEMO_WINDOWS_AUTHENTICODE_STATUS=NOT_VALID"
    }
    if ($RequireSigned) {
        throw "Authenticode signing was configured, but the application or NSIS installer signature is not valid."
    }
    Write-Host "AUTHENTICODE: not valid; allowed only because signing secrets were not configured."
}
