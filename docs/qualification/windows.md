# Windows qualification (M15-A)

Preparation and gated native runtime suite for real Windows hosts.

This document is **preparation + harness**. It does **not** claim native Windows
runtime PASS until a genuine Windows machine/runner executes the suite.

## Quick start

### From any host (prep / packaging checks)

```bash
npm run windows:qualification:prep
```

On macOS/Linux the harness verifies packaging configuration, records an
environment report, and marks every native runtime section `NOT RUN`.
Skipped sections are never auto-marked `PASS`.

### From GitHub Actions (maintainer on macOS)

Manual workflow **ZEMO Windows Private Beta**
(`.github/workflows/zemo-windows-private-beta.yml`) runs the same harness on
`windows-latest`. Maintainer steps: `docs/beta/windows-github-actions.md`.

The workflow does **not** claim native PASS until that run finishes. Windows
Apply stays **NOT QUALIFIED** in source docs until then.

### From a real Windows host

Prerequisites:

- Windows 10/11, local NTFS volume for mutation tests
- Rust stable with `x86_64-pc-windows-msvc` (Visual Studio Build Tools / MSVC)
- Node.js 22+
- Optional but required for SEMANTIC PASS: provisioned Granite assets via
  `SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR` pointing at a directory that contains
  `onnx/model_quint8_avx2.onnx` and `tokenizer.json`

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\windows-qualification.ps1
# or
npm run windows:qualification
```

Reports are written to `target/windows-qualification/`.

## What the harness runs

### READ-ONLY

Startup-adjacent scan, workspace restore paths, extraction sandbox, lexical
search, identities, proposals, rules, and UI smoke hooks that are safe against
a temporary sandbox only.

### SEMANTIC

Model install/checksum activation, ORT load, tokenizer load, real Granite
embedding, ANN insert/query/reload, model removal, and lexical fallback.
Uses the real pinned Granite package. If the model directory is unset the
section is `NOT RUN` (not PASS).

### MONITORING

Native watcher qualification (`crates/platform/tests/windows_watcher_qualification.rs`)
for create / modify / rename / delete / directory rename / burst / restart recovery /
path encoding / local volume distinction. Backend expected on Windows:
`ReadDirectoryChanges` via `notify::RecommendedWatcher`.

### EXECUTOR / NTFS / ROLLBACK

M8 executor suites plus `platform-windows` NTFS qualification (`--features mutation`):

- same-volume move, rename, case-only rename (staged), collision
- long path, locked file, read-only file, ACL denied
- junction / reparse / symlink refusal
- rollback / interrupted execution / restart reconciliation / journal recovery

Safety policy is not weakened for portability.

## Suites prepared

| Area | Location |
| --- | --- |
| NTFS / executor primitives | `crates/platform-windows/tests/ntfs_qualification.rs` |
| Native paths / Unicode / reserved | `crates/platform-windows/tests/windows_native_paths.rs` |
| ORT / Granite / USearch | `crates/search/tests/windows_runtime_qualification.rs` |
| Watcher / monitoring | `crates/platform/tests/windows_watcher_qualification.rs` |
| Sandbox containment | `crates/platform/tests/windows_sandbox_safety.rs` |
| Read-only product flow | `crates/application/tests/windows_read_only_qualification.rs` |

The read-only application suite requires a compiling `application` crate. If a
parallel milestone temporarily breaks that package, the harness will report FAIL
honestly for READ-ONLY rather than marking PASS.

## Sandbox safety

Qualification must never use real Documents, Desktop, Downloads, business
files, or system directories. Fixtures use OS temporary directories with prefixes:

- `supremacy-m15-sandbox-*` (M15 harness)
- `supremacy-m8-sandbox-*` (existing M8 suites)

Containment is asserted before mutation.

## Installer qualification checklist (release-like)

On a Windows install / packaged build, verify:

1. App launches without developer shell env vars
2. `operation-executor-*.exe` sidecar is found beside the app binaries
3. ORT native runtime is available from the build-time `download-binaries` path
4. Model storage under app local data is writable
5. ANN storage under app local data is writable
6. SQLCipher DB directory is writable
7. No requirement for `SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR` for normal users
   (offline/dev only)

DB writable and path probes are automated by the harness on Windows hosts.

## Report format

```
WINDOWS QUALIFICATION

ENVIRONMENT:
  ...

READ-ONLY: PASS | FAIL | PARTIAL | NOT RUN
SEMANTIC: ...
MONITORING: ...
EXECUTOR: ...
NTFS: ...
ROLLBACK: ...
INSTALLER: ...
SANDBOX SAFETY: ...

NATIVE WINDOWS RUNTIME: NOT TESTED | RUN ATTEMPTED
```

## Cross-compilation note

`cargo check --target x86_64-pc-windows-msvc` may be attempted from macOS when
the target is installed. Missing MSVC `lib.exe` / link tooling is reported as
`PARTIAL` for build prep and is **not** native runtime qualification.

## Do not touch during M15-A

Harness / docs / Windows-gated tests / packaging only. Do not refactor proposal
engine, OCR/PDF, monitoring business behavior, or M8 architecture except for
tiny Windows buildability fixes.
