# Fail-closed NTFS gate for Windows qualification.
# Creates a disposable sandbox under the runner temp volume and refuses
# mutation qualification when that volume is not NTFS.
#
# Do not read the PowerShell provider name from Get-Item; that value is
# often empty on GitHub-hosted runner temp paths and is not the volume type.

$ErrorActionPreference = "Stop"

function Get-ZemoWindowsVolumeInfo {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $resolved = $Path
    if (Test-Path -LiteralPath $Path) {
        $resolvedItem = Resolve-Path -LiteralPath $Path
        if ($resolvedItem.ProviderPath) {
            $resolved = [string]$resolvedItem.ProviderPath
        } else {
            $resolved = [string]$resolvedItem.Path
        }
    } else {
        $resolved = [System.IO.Path]::GetFullPath($Path)
    }

    $volumeRoot = [System.IO.Path]::GetPathRoot($resolved)
    if ([string]::IsNullOrWhiteSpace($volumeRoot)) {
        throw "cannot determine volume root for '$resolved'"
    }

    $driveLetter = ($volumeRoot.TrimEnd('\', '/') -replace ':$', '')
    $filesystem = ""
    $source = ""

    try {
        $driveInfo = [System.IO.DriveInfo]::new($volumeRoot)
        $format = [string]$driveInfo.DriveFormat
        if (-not [string]::IsNullOrWhiteSpace($format)) {
            $filesystem = $format
            $source = "DriveInfo.DriveFormat"
        }
    } catch {
        # Fall through to Get-Volume / CIM.
    }

    if ([string]::IsNullOrWhiteSpace($filesystem) -and $driveLetter -match '^[A-Za-z]$') {
        try {
            $volume = Get-Volume -DriveLetter $driveLetter -ErrorAction Stop
            $fromVolume = [string]$volume.FileSystemType
            if ([string]::IsNullOrWhiteSpace($fromVolume)) {
                $fromVolume = [string]$volume.FileSystem
            }
            if (-not [string]::IsNullOrWhiteSpace($fromVolume)) {
                $filesystem = $fromVolume
                $source = "Get-Volume"
            }
        } catch {
            # Fall through to CIM.
        }
    }

    if ([string]::IsNullOrWhiteSpace($filesystem) -and $driveLetter -match '^[A-Za-z]$') {
        $device = "${driveLetter}:"
        $disk = Get-CimInstance -ClassName Win32_LogicalDisk -Filter "DeviceID='$device'" -ErrorAction Stop
        $fromCim = [string]$disk.FileSystem
        if (-not [string]::IsNullOrWhiteSpace($fromCim)) {
            $filesystem = $fromCim
            $source = "Win32_LogicalDisk"
        }
    }

    return [pscustomobject]@{
        Sandbox = $Path
        ResolvedPath = $resolved
        VolumeRoot = $volumeRoot
        DriveLetter = $driveLetter
        Filesystem = $filesystem
        DetectionSource = $source
    }
}

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

$info = Get-ZemoWindowsVolumeInfo -Path $root
$filesystem = ([string]$info.Filesystem).Trim()

Write-Host "Qualification sandbox: $($info.Sandbox)"
Write-Host "Resolved path: $($info.ResolvedPath)"
Write-Host "Volume root: $($info.VolumeRoot)"
Write-Host "Drive letter: $($info.DriveLetter)"
Write-Host "Filesystem: $filesystem"
if ($info.DetectionSource) {
    Write-Host "Detection source: $($info.DetectionSource)"
}

if ([string]::IsNullOrWhiteSpace($filesystem)) {
    Write-Error "Qualification volume filesystem could not be determined. Apply qualification must not run."
    exit 1
}

$normalized = $filesystem.ToUpperInvariant()
if ($normalized -eq "FILESYSTEM") {
    Write-Error "Refusing PowerShell provider name 'FileSystem'; that is not a volume type. Apply qualification must not run."
    exit 1
}
if ($normalized -ne "NTFS") {
    Write-Error "Qualification volume is '$filesystem', not NTFS. Apply qualification must not run."
    exit 1
}

if ($env:GITHUB_ENV) {
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "TEMP=$($info.ResolvedPath)"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "TMP=$($info.ResolvedPath)"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "TMPDIR=$($info.ResolvedPath)"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "ZEMO_WINDOWS_QUALIFICATION_ROOT=$($info.ResolvedPath)"
    Add-Content -LiteralPath $env:GITHUB_ENV -Value "ZEMO_WINDOWS_QUALIFICATION_FILESYSTEM=$filesystem"
}

if ($env:GITHUB_OUTPUT) {
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "filesystem=$filesystem"
    Add-Content -LiteralPath $env:GITHUB_OUTPUT -Value "sandbox=$($info.ResolvedPath)"
}

Write-Host "NTFS gate: PASS"
exit 0
