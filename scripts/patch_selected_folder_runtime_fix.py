from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def patch_one(path: str, old: str, new: str, label: str) -> None:
    target = ROOT / path
    text = target.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected 1 match, got {count}")
    target.write_text(text.replace(old, new, 1), encoding="utf-8")


# Folder-bundle policy may change the destination/reason, but semantic_context is a
# bounded semantic field persisted with a strict DB CHECK. Do not overload it with
# an internal policy marker such as "consumer_folder_bundle"; preserve the original
# personal/business/mixed/unknown value produced for the file.
patch_one(
    "crates/organizer/src/organization.rs",
    '        draft.operation.semantic_context = "consumer_folder_bundle".to_owned();\n',
    '        // Preserve the original bounded semantic context; bundle policy only changes placement.\n',
    "consumer bundle semantic context",
)

# A folder explicitly selected by the user must become a live monitored root right
# away, not merely have monitoring metadata stored for a later dashboard visit.
patch_one(
    "crates/application/src/scanner.rs",
    "            self.ensure_root_monitoring_metadata(workspace_id, existing.id)?;\n            return Ok(existing);",
    "            self.register_root_for_monitoring(&existing)?;\n            return Ok(existing);",
    "existing selected root monitoring",
)
patch_one(
    "crates/application/src/scanner.rs",
    "        self.ensure_root_monitoring_metadata(workspace_id, root.id)?;\n        Ok(root)",
    "        self.register_root_for_monitoring(&root)?;\n        Ok(root)",
    "new selected root monitoring",
)

print("selected-folder runtime invariants patched")
