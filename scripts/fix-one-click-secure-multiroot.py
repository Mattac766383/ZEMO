from pathlib import Path


def patch_once(path: str, old: str, new: str) -> None:
    target = Path(path)
    text = target.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}")
    target.write_text(text.replace(old, new, 1))


persistence = Path("crates/persistence/src/lib.rs")
text = persistence.read_text()
if "pub fn root_by_id(" not in text:
    anchor = "    pub fn active_root(&self, workspace_id: WorkspaceId) -> Result<RootRecord, PersistenceError> {\n"
    method = '''    pub fn root_by_id(\n        &self,\n        workspace_id: WorkspaceId,\n        root_id: RootId,\n    ) -> Result<RootRecord, PersistenceError> {\n        let connection = self.lock()?;\n        connection\n            .query_row(\n                \"SELECT display_name, absolute_path, absolute_path_native\n                 FROM roots\n                 WHERE id = ?1 AND workspace_id = ?2 AND state = 'active'\",\n                params![root_id.to_string(), workspace_id.to_string()],\n                |row| {\n                    Ok((\n                        row.get::<_, String>(0)?,\n                        row.get::<_, String>(1)?,\n                        row.get::<_, Vec<u8>>(2)?,\n                    ))\n                },\n            )\n            .optional()?\n            .map(\n                |(display_label, absolute_path, absolute_path_native)| -> Result<RootRecord, PersistenceError> {\n                    Ok(RootRecord {\n                        id: root_id,\n                        workspace_id,\n                        display_label,\n                        absolute_path,\n                        absolute_path_native: monitoring::decode_native_path(&absolute_path_native)?,\n                    })\n                },\n            )\n            .transpose()?\n            .ok_or(PersistenceError::NotFound)\n    }\n\n'''
    if anchor not in text:
        raise SystemExit("persistence active_root anchor missing")
    persistence.write_text(text.replace(anchor, method + anchor, 1))

execution = Path("crates/application/src/execution.rs")
text = execution.read_text()
old = '''        let root = self.database.active_root(proposal.workspace_id)?;\n        if root.id != proposal.root_id {\n            return Err(ApplicationError::ExecutionApprovalRequired);\n        }'''
new = '''        let root = self\n            .database\n            .root_by_id(proposal.workspace_id, proposal.root_id)?;'''
if old in text:
    text = text.replace(old, new, 1)
elif "root_by_id(proposal.workspace_id, proposal.root_id)" not in text:
    raise SystemExit("prepare_execution root binding anchor missing")

old = '''        let root_record = match self.database.active_root(detail.session.workspace_id) {'''
new = '''        let root_record = match self\n            .database\n            .root_by_id(detail.session.workspace_id, detail.session.root_id)\n        {'''
if old in text:
    text = text.replace(old, new, 1)
elif "root_by_id(detail.session.workspace_id, detail.session.root_id)" not in text:
    raise SystemExit("consent root revalidation anchor missing")
execution.write_text(text)

app = Path("apps/desktop/src/App.tsx").read_text()
for symbol in (
    "prepareExecution(current.id, current.revision)",
    "approveExecution(prepared.session.id)",
    "startExecution(approved.session.id)",
    "rollbackExecution(executionId)",
):
    if symbol not in app:
        raise SystemExit(f"secure One-Click path missing: {symbol}")

print("secure multi-root patch applied")
