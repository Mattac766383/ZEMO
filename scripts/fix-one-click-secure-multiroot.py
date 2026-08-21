from pathlib import Path

# Version marker: changing this intentionally retriggers the PR repair workflow.
REPAIR_VERSION = 3


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one anchor, found {count}: {old[:120]!r}")
    p.write_text(text.replace(old, new, 1))


# The consumer One-Click flow registers several standard folders in the same
# workspace. Execution must bind to the proposal's root_id, never to whichever
# root happened to be registered last.
persistence = Path("crates/persistence/src/lib.rs")
s = persistence.read_text()
if "pub fn root_by_id(" not in s:
    anchor = "    pub fn active_root(&self, workspace_id: WorkspaceId) -> Result<RootRecord, PersistenceError> {\n"
    method = '''    pub fn root_by_id(\n        &self,\n        workspace_id: WorkspaceId,\n        root_id: RootId,\n    ) -> Result<RootRecord, PersistenceError> {\n        let connection = self.lock()?;\n        connection\n            .query_row(\n                \"SELECT display_name, absolute_path, absolute_path_native\n                 FROM roots\n                 WHERE id = ?1 AND workspace_id = ?2 AND state = 'active'\",\n                params![root_id.to_string(), workspace_id.to_string()],\n                |row| {\n                    Ok((\n                        row.get::<_, String>(0)?,\n                        row.get::<_, String>(1)?,\n                        row.get::<_, Vec<u8>>(2)?,\n                    ))\n                },\n            )\n            .optional()?\n            .map(\n                |(display_label, absolute_path, absolute_path_native)| -> Result<\n                    RootRecord,\n                    PersistenceError,\n                > {\n                    Ok(RootRecord {\n                        id: root_id,\n                        workspace_id,\n                        display_label,\n                        absolute_path,\n                        absolute_path_native: monitoring::decode_native_path(\n                            &absolute_path_native,\n                        )?,\n                    })\n                },\n            )\n            .transpose()?\n            .ok_or(PersistenceError::NotFound)\n    }\n\n'''
    if anchor not in s:
        raise SystemExit("persistence active_root anchor missing")
    s = s.replace(anchor, method + anchor, 1)
    persistence.write_text(s)

execution = Path("crates/application/src/execution.rs")
s = execution.read_text()
old = '''        let root = self.database.active_root(proposal.workspace_id)?;\n        if root.id != proposal.root_id {\n            return Err(ApplicationError::ExecutionApprovalRequired);\n        }'''
new = '''        let root = self\n            .database\n            .root_by_id(proposal.workspace_id, proposal.root_id)?;'''
if old in s:
    s = s.replace(old, new, 1)
elif "root_by_id(proposal.workspace_id, proposal.root_id)" not in s:
    raise SystemExit("prepare_execution root binding anchor missing")

old = '''        let root_record = match self.database.active_root(detail.session.workspace_id) {'''
new = '''        let root_record = match self\n            .database\n            .root_by_id(detail.session.workspace_id, detail.session.root_id)\n        {'''
if old in s:
    s = s.replace(old, new, 1)
elif "root_by_id(detail.session.workspace_id, detail.session.root_id)" not in s:
    raise SystemExit("consent root revalidation anchor missing")

execution.write_text(s)

# Guard the primary UI route: standard personal folders must keep using the
# metadata-only consumer proposal mode and the existing approved executor.
app = Path("apps/desktop/src/App.tsx")
app_text = app.read_text()
required = '''        const proposal = await generateOrganizationProposal(\n          workspaceId,\n          false,\n          outcome.root.id,\n          true,\n        );'''
if required not in app_text:
    raise SystemExit("One-Click UI no longer requests consumer-mode proposals")
for required_symbol in (
    "prepareExecution(current.id, current.revision)",
    "approveExecution(prepared.session.id)",
    "startExecution(approved.session.id)",
    "rollbackExecution(executionId)",
):
    if required_symbol not in app_text:
        raise SystemExit(f"secure One-Click execution path missing: {required_symbol}")

print(f"secure multi-root One-Click patch applied (repair v{REPAIR_VERSION})")