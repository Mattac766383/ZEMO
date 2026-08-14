# Private beta — macOS Apply + Undo (5–10 users)

ZEMO — controlled private beta pack **0.1.0-beta.5** (app version **0.1.0**).

**DISTRIBUTE beta.5 only.** Older Working Name packs are superseded.

A copy in `/Applications/Working Name.app` from an earlier pack is **not** this build. Replace it with **ZEMO** from the beta.5 DMG.

Packaged application name: **ZEMO**  
Bundle identifier: `com.workingname.organizer` (unchanged — app data / Keychain / executor auth)  
Architecture: **arm64 (Apple Silicon) only** — not Intel, not universal.

## Scope

**Supported**

- Onboarding (Organiser mon ordinateur / Choisir des dossiers), scan, extraction, semantic understanding
- Accueil, Organisation, Recherche, Surveillance (À revoir / préférences / inventaire under Options avancées)
- Organization proposal preview, TO_REVIEW, review corrections
- Lexical search; optional local Granite model install + hybrid/ANN
- Monitoring (proposal-only), rules & preferences

**Not enabled / not beta-safe**

- Delete / duplicate cleanup Apply
- Automatic mutation of any kind
- Cross-volume moves (fail-closed)
- Windows Apply (native qualification **NOT TESTED**)

Apply moves and renames only files in the approved proposal, after confirmation. Undo is available when the previous location is free.

## Distribution artifacts

Maintainer output (after `npm run tauri -- build`):

- App bundle: `artifacts/zemo-beta/ZEMO.app`
- DMG (distribute this): `artifacts/zemo-beta/ZEMO-0.1.0-beta.5-arm64.dmg`
- Superseded (do not distribute): older `0.1.0-beta.1` / `beta.2` / `beta.3-m18` / `beta.3-m18.1` packs
- Checksums: `artifacts/private-beta/SHA256SUMS.txt`
- Build info: `artifacts/private-beta/BUILDINFO.txt`

Tauri also writes bundles under its Cargo target `release/bundle/` (environment-specific). Distribute only `artifacts/private-beta/`.

Give testers only the DMG + `README-FIRST.txt` (checksums in `SHA256SUMS.txt`).  
Do not send source, `target/`, `node_modules`, `.env`, or databases.

## Signing / notarization

- **SIGNING:** NOT CONFIGURED for distribution. No Developer ID Application identity. A local Apple Development certificate must **not** be used for this pack. The release bundle is produced with Tauri `signingIdentity: "-"`.
- **NOTARIZATION:** NOT CONFIGURED.

Do not call ad-hoc signing “production signing”. Gatekeeper warnings are expected.

## Installation (unsigned private beta)

1. Open the `.dmg`.
2. Drag **ZEMO** to **Applications**.
3. Open the app from Applications.
4. If macOS blocks it: Finder → Applications → right-click / control-click **ZEMO** → **Open** → confirm **Open**.
5. First launch starts with **Organiser mon ordinateur** or **Choisir des dossiers**. Permission prompts should appear only for the locations included in the chosen scope — not before that choice. Testers may decline an inaccessible folder.
6. Optional semantic search: in **Recherche**, explicitly install the pinned local model (~118 MiB). No env var required. Lexical search works without it.

Do **not** tell testers to disable Gatekeeper (`spctl --master-disable` or equivalent).

## Application data (actual paths)

Derived from Tauri `app_local_data_dir` + identifier `com.workingname.organizer`:

| What | Path |
| --- | --- |
| Database (SQLCipher) | `~/Library/Application Support/com.workingname.organizer/catalog.db` |
| Execution journal | `~/Library/Application Support/com.workingname.organizer/operation-recovery.jsonl.enc` |
| Semantic model | `~/Library/Application Support/com.workingname.organizer/models/embeddings/` |
| ANN index | `~/Library/Application Support/com.workingname.organizer/models/embeddings/ann/` |
| Database key | macOS Keychain (`catalog-database-key-v1` via the app secret store) |
| Onboarding flag | WebView local storage (`supremacy.onboarding.v1.completed`) |
| Dedicated log file | **None** (no rotating product log in this build) |

WebView / cache may also appear under standard Apple containers for the same identifier.

## Uninstall

**A. Remove the application** (does **not** delete user documents):

- Drag **ZEMO** from Applications to Trash, then empty Trash if desired.

**B. Remove application data** (optional, separate, still does **not** delete the scanned folder):

- `~/Library/Application Support/com.workingname.organizer/`
- Keychain entries created by the app (catalog key / executor root), if present
- WebView site data for `com.workingname.organizer` if it remains

If Apply ran, uninstalling the app does **not** move files back. Use **Annuler les changements** in the app when it is still available. User documents are never deleted by uninstall.

Do not run a recursive delete script against unknown paths.

## Diagnostics

Prefer on-screen errors and the feedback template. There is **no** sanitized diagnostic export yet.

Testers should send:

- App version from the header
- Mac model + macOS version
- Short description of the last action
- Exact error banner text (excerpt)

They should **not** send `catalog.db`, model weights, or original files.

**Beta issue:** scan/review UI can show relative paths and extracted snippets. Ask testers to crop screenshots. Console.app may contain paths if the process printed to stderr — treat that as sensitive.

## OCR / PDF host tools

Tesseract and pdftoppm are **not** vendored. Discovery is env override → bundled sidecar (none in this pack) → trusted host paths (`/opt/homebrew/bin`, `/usr/local/bin`, `/usr/bin`). Missing tools → partial/unavailable extraction, documents may land in review — the app must still launch.

## Semantic model

Normal testers must **not** set `SUPREMACY_LOCAL_EMBEDDING_MODEL_DIR`.  
UI path: Recherche → **Activer la recherche sémantique locale** → pinned HTTPS download → SHA-256 verify → install under app data.

## Tester safety

Use a **copy** of a messy personal folder first — not the sole critical business archive.

## Tester materials

- [Tester guide](tester-guide.md)
- [Feedback template](feedback-template.md)
- [Maintainer session checklist](test-session-checklist.md)

## Beta tasks

1. Install and launch the **beta.5** DMG.
2. Start with **Organiser mon ordinateur** (user-content folders only) or **Choisir des dossiers** (prefer a **copy** of a messy folder).
3. Run scan (+ analysis if prompted).
4. Inspect Accueil, then organization proposal (current vs proposed).
5. Find one wrong proposal and correct/review it.
6. Search for a document without relying on the filename.
7. Add a new file into the watched folder.
8. On a disposable test folder, Apply the organization, verify files moved, then Undo.
9. Observe monitoring update the proposal (monitoring never auto-applies).
10. Note crashes, confusion, or mistrust.
