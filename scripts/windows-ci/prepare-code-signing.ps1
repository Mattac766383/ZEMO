param(
    [string]$OutputPath
)

$ErrorActionPreference = "Stop"

function Write-CiValue {
    param(
        [string]$Name,
        [string]$Value,
        [switch]$Output,
        [switch]$Environment
    )
    if ($Output -and $env:GITHUB_OUTPUT) {
        Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "$Name=$Value"
    }
    if ($Environment -and $env:GITHUB_ENV) {
        Add-Content -LiteralPath $env:GITHUB_ENV -Value "$Name=$Value"
    }
}

$certificateBase64 = [string]$env:WINDOWS_CERTIFICATE
$certificatePassword = [string]$env:WINDOWS_CERTIFICATE_PASSWORD
$timestampUrl = [string]$env:WINDOWS_TIMESTAMP_URL
if ([string]::IsNullOrWhiteSpace($timestampUrl)) {
    $timestampUrl = "http://timestamp.digicert.com"
}

$hasCertificate = -not [string]::IsNullOrWhiteSpace($certificateBase64)
$hasPassword = -not [string]::IsNullOrWhiteSpace($certificatePassword)

if (-not $hasCertificate -and -not $hasPassword) {
    Write-Host "WINDOWS SIGNING: certificate secrets are not configured; build remains unsigned."
    Write-CiValue -Name "enabled" -Value "false" -Output
    Write-CiValue -Name "ZEMO_WINDOWS_SIGNING_ENABLED" -Value "false" -Environment
    exit 0
}

if ($hasCertificate -xor $hasPassword) {
    throw "WINDOWS_CERTIFICATE and WINDOWS_CERTIFICATE_PASSWORD must either both be configured or both be absent."
}

if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $tempRoot = if (-not [string]::IsNullOrWhiteSpace($env:RUNNER_TEMP)) {
        $env:RUNNER_TEMP
    } else {
        [System.IO.Path]::GetTempPath()
    }
    $OutputPath = Join-Path $tempRoot "zemo-tauri-windows-signing.json"
}

$workRoot = Join-Path ([System.IO.Path]::GetDirectoryName($OutputPath)) "zemo-windows-signing"
New-Item -ItemType Directory -Force -Path $workRoot | Out-Null
$encodedPath = Join-Path $workRoot "certificate.base64"
$pfxPath = Join-Path $workRoot "certificate.pfx"

try {
    Set-Content -LiteralPath $encodedPath -Value $certificateBase64 -Encoding ascii -NoNewline

    & certutil.exe -f -decode $encodedPath $pfxPath | Out-Null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $pfxPath)) {
        $cleanBase64 = ($certificateBase64 -replace "-----BEGIN CERTIFICATE-----", "" -replace "-----END CERTIFICATE-----", "" -replace "\s", "")
        [System.IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($cleanBase64))
    }

    $securePassword = ConvertTo-SecureString -String $certificatePassword -AsPlainText -Force
    $imported = @(Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation "Cert:\CurrentUser\My" -Password $securePassword)
    if (-not $imported -or $imported.Count -eq 0) {
        throw "The PFX did not import any certificates."
    }

    $codeSigningOid = "1.3.6.1.5.5.7.3.3"
    $certificate = $imported |
        Where-Object {
            $_.HasPrivateKey -and
            ($_.EnhancedKeyUsageList | Where-Object { $_.ObjectId.Value -eq $codeSigningOid })
        } |
        Select-Object -First 1

    if (-not $certificate) {
        throw "The imported PFX does not contain a private-key certificate with the Code Signing EKU."
    }

    $thumbprint = ($certificate.Thumbprint -replace "\s", "").ToUpperInvariant()
    if ([string]::IsNullOrWhiteSpace($thumbprint)) {
        throw "Imported code-signing certificate has no thumbprint."
    }

    $config = [ordered]@{
        bundle = [ordered]@{
            windows = [ordered]@{
                certificateThumbprint = $thumbprint
                digestAlgorithm = "sha256"
                timestampUrl = $timestampUrl
                tsp = $true
            }
        }
    }

    New-Item -ItemType Directory -Force -Path ([System.IO.Path]::GetDirectoryName($OutputPath)) | Out-Null
    ($config | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $OutputPath -Encoding utf8

    Write-Host "WINDOWS SIGNING: Authenticode certificate imported for this ephemeral runner."
    Write-Host "WINDOWS SIGNING: SHA-256 + RFC3161 timestamp configured."
    Write-Host "WINDOWS SIGNING: certificate subject $($certificate.Subject)"

    Write-CiValue -Name "enabled" -Value "true" -Output
    Write-CiValue -Name "config_path" -Value $OutputPath -Output
    Write-CiValue -Name "thumbprint" -Value $thumbprint -Output
    Write-CiValue -Name "ZEMO_WINDOWS_SIGNING_ENABLED" -Value "true" -Environment
    Write-CiValue -Name "ZEMO_TAURI_SIGNING_CONFIG" -Value $OutputPath -Environment
    Write-CiValue -Name "ZEMO_WINDOWS_CERT_THUMBPRINT" -Value $thumbprint -Environment
}
finally {
    Remove-Item -LiteralPath $encodedPath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $pfxPath -Force -ErrorAction SilentlyContinue
}
