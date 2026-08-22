# ZEMO One-Click v3 — rebuild contract

## Goal
One click must produce a visibly cleaner personal folder, without touching applications, system components, shortcuts, symlinks, cloud placeholders, or unsafe files. A green UI is never enough: success requires verified physical filesystem moves.

## Architecture

1. Discover
   - Enumerate the selected personal root until exhaustion or explicit cancellation.
   - No arbitrary file-count or wall-clock cutoff.
   - Bounded memory: persist/index in batches rather than accumulating the whole folder in RAM.
   - Emit throttled progress with discovered/indexed/skipped/error counts.

2. Decide
   - Deterministic baseline policy, no AI dependency.
   - Programs/system files/shortcuts stay in place.
   - Loose personal files are always assigned to a shallow destination or `À vérifier`.
   - Filename/root/extension/date heuristics may refine categories; semantics may refine later but may never block baseline cleanup.

3. Plan
   - Persist one exact proposal per root.
   - Every proposed move has a real source file/version identity from the scan.
   - Collisions are resolved before Apply.
   - Preview counts must equal the exact move operations that Apply will attempt.

4. Apply
   - Use only the approved authenticated sidecar path.
   - Never mutate from the renderer/Tauri process directly.
   - Success requires `applied == proposedMoves` and `failed == 0` for every root.
   - Partial execution is surfaced as partial/failure, never as `ordinateur rangé`.

5. Undo
   - Reverse all completed executions in reverse root order.
   - Verify rollback result and surface partial rollback explicitly.

6. Runtime proof
   - Core tests are insufficient.
   - CI must build the actual Tauri bundle, verify the packaged sidecar, launch the authenticated sidecar from the bundle, perform physical moves, and Undo them.
   - Windows packaging gets an equivalent package-level qualification before beta release.

## Product behavior

### Baseline categories
- Documents/Administratif/Factures
- Documents/Administratif/Banque
- Documents/Administratif/Assurances
- Documents/Administratif/Impôts
- Documents/Travail
- Documents/Études
- Documents/Personnel
- Images/Captures d’écran
- Images/Photos
- Images/Images téléchargées
- Vidéos
- Archives
- Installateurs
- Code (only loose source files)
- À vérifier

### Existing directories
Existing root-level directories are not silently moved in v3 baseline. Moving a directory can invalidate project/application paths and the current approved execution policy fingerprints regular files, not directory trees. The UI must report how many directories were intentionally left in place. A future folder-moving mode requires a separate directory-tree fingerprint and rollback policy.

## Mandatory acceptance tests

- 0 files: clean no-op, no false success.
- 1 file of every supported class.
- 10,000+ loose files: no truncation, bounded-memory scan.
- Name collisions.
- Unicode and very long names within OS limits.
- Hidden files, symlinks, app bundles, DLL/dylib/system components remain untouched.
- Locked/source-changed files fail closed and do not produce a success screen.
- Multi-root Desktop + Downloads + Documents keeps exact root identity through Apply.
- Physical Apply changes disk state.
- Undo restores byte-for-byte initial fixture state.
- Actual packaged macOS app sidecar test.
- Actual packaged Windows installer/runtime sidecar test before public beta.
