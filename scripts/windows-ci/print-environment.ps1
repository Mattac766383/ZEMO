# Record the actual GitHub Windows runner environment. Do not print secrets.

$ErrorActionPreference = "Continue"

Write-Host "=== ZEMO Windows runner environment ==="
Write-Host "OS caption: $((Get-CimInstance Win32_OperatingSystem | Select-Object -First 1).Caption)"
Write-Host "Windows version: $([System.Environment]::OSVersion.VersionString)"
Write-Host "Architecture: $env:PROCESSOR_ARCHITECTURE"
Write-Host "Process arch: $([System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture)"
Write-Host "OS arch: $([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
Write-Host "Image OS: $env:ImageOS"
Write-Host "Image version: $env:ImageVersion"
Write-Host "Runner OS: $env:RUNNER_OS"
Write-Host "Runner arch: $env:RUNNER_ARCH"
Write-Host "Runner temp: $env:RUNNER_TEMP"
Write-Host "Workspace: $env:GITHUB_WORKSPACE"

Write-Host "--- Rust ---"
rustc --version
rustc -vV
cargo --version
rustup show

Write-Host "--- Node ---"
node --version
npm --version

Write-Host "--- MSVC / Windows SDK ---"
$link = Get-Command link.exe -ErrorAction SilentlyContinue
$cl = Get-Command cl.exe -ErrorAction SilentlyContinue
Write-Host "link.exe: $(if ($link) { $link.Source } else { 'MISSING' })"
Write-Host "cl.exe: $(if ($cl) { $cl.Source } else { 'MISSING' })"

Write-Host "--- WebView2 ---"
$webview = Get-ItemProperty -Path "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue
if ($webview) {
    Write-Host "WebView2 pv: $($webview.pv)"
} else {
    Write-Host "WebView2 registry key not found (windows-latest still typically provides the runtime)"
}

Write-Host "WINDOWS SIGNING: not printed; detect-only in a later step"
Write-Host "SMARTSCREEN USER EXPERIENCE: NOT QUALIFIED"
Write-Host "=== end environment ==="
