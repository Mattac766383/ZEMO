# ZEMO Windows private beta — GitHub Actions (from macOS)

The maintainer develops on macOS. A real Windows installer is produced by
GitHub-hosted `windows-latest` (x64 / MSVC). Do not cross-compile from macOS
and do not treat a Mac run as native Windows qualification.

Workflow: **ZEMO Windows Private Beta**
Path: `.github/workflows/zemo-windows-private-beta.yml`
Trigger: **workflow_dispatch only** (Actions → Run workflow).
No public GitHub Release, Store upload, or auto-update.

## Maintainer steps (macOS)

1. Push the branch/commit to GitHub.
2. Open the repository on GitHub.
3. Open **Actions**.
4. Select **ZEMO Windows Private Beta**.
5. Click **Run workflow**.
6. Wait for completion.
7. Scroll to **Artifacts**.
8. Download `ZEMO-windows-private-beta` (Apply-enabled) or, if qualification
   did not unlock Apply, `ZEMO-windows-private-beta-propose-only`.
9. Extract the archive.
10. Send the `.exe` installer to the Windows tester, together with
    `README-FIRST.txt`, `SHA256SUMS.txt`, and `BUILDINFO.txt`.

The maintainer does not need a Windows machine to generate the installer.

## Expected Apply-enabled files

- `ZEMO-0.1.0-beta.6-windows-x64.exe`
- `SHA256SUMS.txt`
- `BUILDINFO.txt`
- `README-FIRST.txt`
- `qualification-summary.txt`

## Apply enablement

Windows Apply is compiled in **only** when job `windows-qualification`
records `apply_qualified=true` after the existing M15-A harness PASSes on
NTFS. There is no workflow input that can force Apply on.

Until the first successful native run:

- WINDOWS APPLY: **NOT QUALIFIED**
- Workflow prepared: **YES**

Runtime results belong in the qualification artifact, not in source docs.

## Signing / SmartScreen

WINDOWS SIGNING: NOT CONFIGURED  
SMARTSCREEN USER EXPERIENCE: NOT QUALIFIED  

Unsigned private-beta installers may show a SmartScreen warning. Testers
must not disable Windows Defender.
