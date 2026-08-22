# ZEMO Automatic Monitoring

Automatic monitoring is an explicit opt-in extension of ZEMO's existing local watcher pipeline. `PRUDENT` remains the default mode.

## Activation contract

Automatic mode can only be enabled after a native confirmation. Activation is refused while startup reconciliation or queued monitoring work is pending, and when Apply is unavailable, recovery is required, or the authenticated execution journal is locked.

## Eligibility contract

For the first automatic version, monitoring claims one due file job per cycle. A fresh organization proposal is built for the current automatic batch instead of reusing the previous pending proposal.

A physical Apply is eligible only when the fresh proposal contains exactly one MOVE or RENAME candidate and that candidate:

- has confidence of at least 92% (or a stricter configured review threshold),
- is not marked for review,
- is not stale,
- has no conflict requiring review.

Everything else stays review-only. Automatic monitoring never auto-executes delete operations.

## Filesystem safety

The watcher never mutates the filesystem directly. Eligible automatic work follows the same secured path as normal organization:

proposal approval → execution preflight → authenticated executor consent → packaged sidecar Apply → authenticated journal.

Existing no-overwrite, source-drift, protected-path, filesystem qualification, recovery and rollback checks remain authoritative. If any execution gate is unavailable, automatic work fails closed instead of bypassing it.

## User control

The Surveillance screen exposes the current mode and a control to return immediately to Prudent mode. Automatic actions remain visible through normal execution history and can be undone when rollback is available.
