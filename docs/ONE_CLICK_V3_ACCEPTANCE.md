# One-Click v3 acceptance matrix

A change is not considered a fix unless the relevant row is proven by automation or a recorded package qualification.

| Case | Required result |
| --- | --- |
| Empty root | Preview shows 0 moves; Apply is a no-op; no fake `rangé` success |
| 10,001 loose files | All discoverable safe files indexed; `truncated=false`; bounded progress |
| Desktop + Downloads + Documents | Each proposal stays bound to its own `root_id` through Apply |
| Programs/system files | Never proposed for movement |
| Shortcuts/aliases | Never proposed for movement |
| Unknown loose files | Moved to `À vérifier`, not silently dropped |
| Collision | Deterministic non-overwriting destination |
| Source drift before Apply | Apply refuses changed source |
| One failed move among N | UI reports partial/failure, not full success |
| Apply | Physical source disappearance + destination appearance verified |
| Undo | Initial tree snapshot restored exactly |
| macOS package | Actual `.app` contains executable sidecar and packaged sidecar Apply/Undo passes |
| Windows package | Actual packaged executable/installer contains sidecar and package-level Apply/Undo passes |
