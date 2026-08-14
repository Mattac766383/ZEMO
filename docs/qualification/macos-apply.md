# macOS Apply — sandbox vs packaged permission model (M18)

## Step 1 — sandbox executor (PASS)

See the M18 Step 1 record in `docs/CURRENT_STATE.md`. Mutations stay under
`supremacy-m18-macos-sandbox-*` / `supremacy-m8-sandbox-*`. The app is **not
App Sandboxed**. Mutation is compiled only into `operation-executor`.

## Step 2 — packaged `.app` / `.dmg` (PARTIAL)

Pack tag: **0.1.0-beta.3-m18** (do not distribute). Ad-hoc signing, hardened
runtime, no notarization, no Full Disk Access entitlement, no App Sandbox.

Artifact (local only):

- App: `artifacts/m18-step2/Working Name.app`
- DMG: `artifacts/m18-step2/Working-Name-0.1.0-beta.3-m18-arm64.dmg`
- Architecture: arm64
- Commit at pack time: `066fe57f0b9bff67fd0e7e6434ef2cddf80aebaf` plus uncommitted
  Step 2 handshake/qualification sources

### Packaged executor

The ad-hoc sidecar identity cannot read the app Keychain item. The coordinator
therefore passes a session-scoped 0600 temp file path via
`WORKING_NAME_EXECUTOR_ROOT_FILE` (also mirrored under Application Support).
Missing sidecar fails closed. Handshake + plan-bound Apply succeeded against
the bundled helper.

Renderer capabilities remain `core:default` only (no `fs:` / `shell:`).

### What was proven

Dedicated `supremacy-m18-step2-sandbox-*` folders only (not Documents / Desktop /
Downloads):

- same-volume move, rename, move+rename
- no-overwrite, source drift, symlink escape
- Unicode / spaces / emoji paths
- POSIX permission deny and chmod-0 revoke-after-preview fail closed
- journal stays in app-data temp dirs, not in scanned folders
- Undo of move/rename; external edit blocks unsafe Undo
- case-only rename Apply succeeds; Undo is `RollbackPartial` (`rollback_blocked=1`)
  on APFS (destination appears to exist under case-insensitive lookup)
- open writable handle: rename still succeeds (not a Windows lock)
- release packaged batches: 10 / 100 / 1000 preflight+Apply+Undo completed

### What was not proven

- Real NSOpenPanel / Files-and-Folders TCC prompt, grant, deny, and persistence
  after relaunch (picker not driven; user TCC.db unreadable without FDA, which
  was not requested)
- Packaged crash-injection during mutation (sandbox Step 1 crash recovery stands;
  journal reopen coherent)
- Cross-volume move, App Sandbox, Developer ID / notarization

### Access model

Unsandboxed app + user-selected / registered roots. Full Disk Access is **not**
required for the tested temp-folder scope and is **not** requested.

TCC still applies to protected locations. That packaged prompt path remains
PARTIAL until a real folder-picker session is observed.
