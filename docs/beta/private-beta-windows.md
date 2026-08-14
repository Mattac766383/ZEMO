# Private beta — Windows (propose-only until NTFS qualification)

ZEMO on Windows is **not Apply-qualified**. Native NTFS mutation has never been
run on a real Windows host in this repository.

Until a native GitHub Windows qualification PASSES:

- Windows testers get **propose-only** (scan, understand, preview, review, search, monitoring).
- **Appliquer l’organisation** stays unavailable.
- Do not organize personal Desktop / Documents / Downloads.

Bundle identifier remains `com.workingname.organizer`.

Installer filename when built by the GitHub Windows workflow after Apply
qualification: `ZEMO-0.1.0-beta.6-windows-x64.exe` (NSIS).

Generate that installer from macOS via Actions — see
`docs/beta/windows-github-actions.md`. This document is **not** a substitute
for a completed native Windows run.
