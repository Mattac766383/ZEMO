# M16-A — PRELIMINARY RELEASE CANDIDATE AUDIT

**Date:** 2026-08-12  
**Scope:** Read-only audit against `docs/CURRENT_STATE.md` + current release surface  
**Parallel work:** M14-A / M15-A may still change the tree — treat as preliminary  
**CODE MODIFIED:** NO

---

## STATUS: CONDITIONAL

Ready for a **propose-only macOS private beta** (scan → extract → understand → preview → review → search → monitoring → rules) **only if** platform Apply limitations and known extraction/search limits are disclosed in-product.

**Not ready** for a private beta whose success criteria include **Apply + rollback on real user files**, because:

1. macOS production Apply is intentionally unavailable (`UnavailableApprovedExecutorClient`).
2. Windows Apply is wired/enabled in the approved-execution host path but native Windows/NTFS runtime remains **NOT TESTED**.

---

## P0

*None confirmed.*

No verified silent overwrite path, automatic Apply, or unexpected document/query upload was found in the audited surface. Safety design (explicit consent, no overwrite, journal-before-mutate, fail-closed recovery) is evidence-backed in CURRENT_STATE / M8.1.

---

## P1

1. **Full beta journey dead-ends at Apply on macOS.** Production desktop uses `UnavailableApprovedExecutorClient` on non-Windows; `ApplyGate::for_approved_execution_host()` disables Apply off Windows. Onboarding/home copy still promises organization “after approval.”
2. **Windows Apply is reachable in code but unqualified.** Native Windows/NTFS, locking, ACL, antivirus, disk-full, ORT/USearch, and monitoring remain NOT TESTED. Shipping Apply to 5–10 real Windows users without that gate is a safety release blocker.
3. **No signed/notarized install distribution evidence.** Tauri bundle is configured (`productName: "Working Name"`, version `0.1.0`); no signing/notarization/updater config found. Private beta install path is developer-oriented (`npm run tauri -- dev` / local build).
4. **First-run trust overclaim risk.** Privacy/onboarding/home imply that approved organization can change files. On macOS that never happens; on Windows it is unproven. Misleading “Apply after approval” is a trust P1 for beta.
5. **Rollback cannot be beta-verified on the current primary (macOS) host.** Rollback exists in the journaled Windows executor path; macOS cannot exercise it in production wiring.

---

## P2

1. **Historique is not a history surface.** Nav “Historique” remaps to Organization / ExecutionPanel — no dedicated apply/rollback history UX.
2. **Execution / approval UI is English + technical** while the primary journey is French (`ExecutionPanel`, proposal footer “Approve reviewed organization”, mixed proposal toolbar).
3. **Static trust chip “Rien n’a encore été modifié”** on Accueil is not tied to whether an Apply ever succeeded (would become false after a real Apply).
4. **OCR depends on host Tesseract/Poppler** (optional, not bundled). Unavailable OCR becomes review items — correct, but beta users may expect OCR without installing system tools.
5. **Extraction gaps remain product-visible:** encrypted PDF, legacy Office, video metadata NOT IMPLEMENTED / unsupported → TO_REVIEW. Acceptable if disclosed; silent confusion if not.
6. **Semantic model RAM footprint is material** (~589 MiB model+ANN in M9.1 evidence). Low-RAM machines may struggle; lexical fallback exists.
7. **No in-app “oublier ce workspace” / local data purge** despite privacy normative docs listing it as an exit criterion.
8. **Single-root folder picker** remains; multi-workspace picker unimplemented.
9. **Monitoring after restart / missed events** rely on reconciliation (PASS in tests), but Windows monitoring unqualified; full one-file pipeline still can trigger root-wide proposal recompute when a current proposal exists (M13 note).
10. **No real-human usability study** (M12 automated walkthrough only).

---

## P3

1. Product still branded **“Working Name”**.
2. Residual English in deeper surfaces (files table headers, some proposal controls).
3. Docs `format-support.md` Phase 0 text still says “pas d’OCR” while the product has optional local OCR — doc drift.
4. No auto-updater (acceptable for private beta if install is manual).
5. Aggressive journal retention/purge automation absent (fail-closed is preferred).
6. Forward resume intentionally NOT IMPLEMENTED (recovery-only) — document for operators.

---

## Journey scores

| Area | Score | Notes |
| --- | --- | --- |
| INSTALLATION | PARTIAL | Bundle config exists; no release packaging/signing/notarization evidence; Windows sidecar prepare path exists |
| ONBOARDING | PARTIAL | Clear local/privacy/scan-safe copy; overpromises Apply capability |
| SCAN | PASS | Read-only scan surface; M12/M13 evidence |
| EXTRACTION | PARTIAL | Partial PDF/OCR; encrypted/legacy/video gaps; review mapping exists |
| SEMANTIC UNDERSTANDING | PASS | Deterministic local M5; conservative TO_REVIEW |
| SEARCH | PASS | Lexical always; optional Granite; install/remove/rebuild UX; explainability |
| ORGANIZATION PREVIEW | PASS | Proposal-only banner; current vs proposed; no Apply in preview itself |
| MONITORING | PARTIAL | Proposal-only, pause/offline volume rejection; Windows unqualified |
| RULES | PASS | Suggestions only; never grants FS permission |
| APPLY SAFETY | PARTIAL | Strong design on Windows path; macOS unavailable; Windows runtime untested |
| ROLLBACK | PARTIAL | Designed + sandbox-qualified; production platform gap |
| RECOVERY | PARTIAL | Fail-closed recovery card exists; macOS cannot complete mutation recovery loops |
| PRIVACY | PASS | No unexpected corpus upload found; model download is pinned HTTPS asset traffic only; no desktop cloud AI wiring found |
| PERFORMANCE | PASS | M13 100k evidence usable for beta scale; proposal ~38 s at 100k is acceptable with UI-bound loads |
| WINDOWS | NOT QUALIFIED | Explicit across CURRENT_STATE |
| MACOS | PARTIAL | Non-mutating product path strong; Apply/rollback unavailable |

---

## Safety evidence (confirmed, not re-architected)

- Explicit approval / plan-bound consent / attestation (M8.1)
- No destination overwrite; no delete/trash Apply ops
- Journal before mutation; rollback revalidates; ambiguous → recovery, no blind resume
- Executor isolation on Windows; UI process has no generic path mutation API
- Monitoring/search/rules have no mutation capability; monitoring `proposal_only: true`
- Non-Windows production Apply client is unavailable

**Confirmed issue for beta:** capability/platform mismatch (trust copy vs actual Apply availability / Windows qualification), not a missing safety architecture.

---

## Search evidence (brief)

- Lexical/structured remain when model unavailable (“vecteurs indisponibles”)
- Consented pinned Hugging Face download; cancel/retry/remove; ANN rebuild UI
- Corruption/incompatible → RebuildRequired + lexical fallback (library-tested)
- Result “Pourquoi ce résultat ?” explainability present

---

## Privacy evidence (brief)

- Desktop does not wire `ai-gateway` cloud inference into the UI path audited
- Embedding install is the intentional network exception (model assets only)
- Catalog/keys via SQLCipher + OS secret store (`keyring`)
- Normative “oublier workspace” purge **not implemented** in app commands (P2)

---

## BETA BLOCKERS

1. Decide beta scope: **propose-only (macOS)** vs **Apply-enabled (Windows qualified)**.
2. If Apply-in-scope: complete native Windows qualification (M8/M8.1 + monitoring + ORT) **or** keep Apply locked.
3. Fix first-run / Accueil trust copy to state platform Apply availability honestly.
4. Provide a concrete install method for 5–10 users (signed build or documented unsigned sideload with consent).
5. Document known limitations in-product or in a beta README: OCR optional, encrypted PDF, legacy Office, video, macOS no Apply, model ~118 MiB download + RAM.

---

## BETA TEST CHECKLIST (5–10 users)

* [ ] Install opens without developer tooling confusion
* [ ] Onboarding completes; privacy claims understood
* [ ] Folder select + first scan succeeds
* [ ] Extraction outcomes understandable (partial / OCR unavailable / encrypted / unsupported)
* [ ] Semantic “understanding” appears without implying certainty
* [ ] Organization preview: Current vs Proposed clear; “not applied yet” believed
* [ ] TO_REVIEW burden acceptable (count + time to clear a sample)
* [ ] False organization rate on a known personal folder is tolerable
* [ ] Search: lexical works offline; optional semantic install/cancel/remove
* [ ] Search results feel trustworthy (“Pourquoi ce résultat ?”)
* [ ] Monitoring: pause, offline root, restart — remains proposal-only
* [ ] Rules: change suggestion only; no FS mutation
* [ ] Apply confidence (Windows-qualified builds only): prepare → consent → apply
* [ ] Rollback after a small approved batch restores prior layout
* [ ] Crash mid-apply (if Apply enabled): recovery card, no silent continue
* [ ] Trust interview: “Did anything move without asking?”
* [ ] Return after 7 days: restore workspace, monitoring health, stale proposal handling
* [ ] Monitoring usefulness after a week of real file churn

---

## RECOMMENDED RELEASE GATE

**Strict but realistic private-beta gate:**

1. **No open P0**
2. **No open safety/privacy P1** (Apply locked **or** Windows Apply path runtime-qualified; no misleading Apply promises)
3. Critical propose-only journey works on macOS: install → onboard → scan → extract → understand → preview → review → search → monitoring → rules
4. Search usable offline (lexical) with honest semantic status
5. Monitoring stable proposal-only (pause/offline/restart)
6. Rollback verified **before** enabling Apply for any beta cohort
7. Known limitations documented for beta users
8. Do **not** require: perfect OCR, video metadata, cross-volume move, 100k real corpuses, WCAG certification, or human-perfect organizer accuracy

**Suggested cohort A (near-term):** macOS propose-only, Apply UI clearly disabled with French explanation.  
**Suggested cohort B (later):** Windows Apply+rollback after native qualification.

---

## NEXT

Wait for M14-A and M15-A results, then run final RC audit.

**STOP.**
