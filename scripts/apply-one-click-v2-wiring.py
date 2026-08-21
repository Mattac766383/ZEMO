from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, got {count}: {old[:100]!r}")
    p.write_text(text.replace(old, new, 1))


# Rust/Tauri wiring.
lib = Path("apps/desktop/src-tauri/src/lib.rs")
s = lib.read_text()
if "mod one_click_v2;" not in s:
    s = s.replace("mod folder_access;\n", "mod folder_access;\nmod one_click_v2;\n", 1)

anchor = "#[tauri::command]\n#[allow(clippy::needless_pass_by_value)]\nfn get_system_status"
commands = '''#[tauri::command]\nasync fn one_click_v2_build_plan(\n    state: State<'_, one_click_v2::OneClickV2State>,\n) -> Result<one_click_v2::OneClickPlan, String> {\n    let runtime = state.inner().clone();\n    tauri::async_runtime::spawn_blocking(move || runtime.build_recommended_plan())\n        .await\n        .map_err(|error| format!("one-click planning task failed: {error}"))?\n}\n\n#[tauri::command]\nasync fn one_click_v2_apply(\n    state: State<'_, one_click_v2::OneClickV2State>,\n) -> Result<one_click_v2::OneClickApplyResult, String> {\n    let runtime = state.inner().clone();\n    tauri::async_runtime::spawn_blocking(move || runtime.apply_current_plan())\n        .await\n        .map_err(|error| format!("one-click apply task failed: {error}"))?\n}\n\n#[tauri::command]\nasync fn one_click_v2_undo(\n    state: State<'_, one_click_v2::OneClickV2State>,\n) -> Result<one_click_v2::OneClickApplyResult, String> {\n    let runtime = state.inner().clone();\n    tauri::async_runtime::spawn_blocking(move || runtime.undo_last())\n        .await\n        .map_err(|error| format!("one-click undo task failed: {error}"))?\n}\n\n#[tauri::command]\n#[allow(clippy::needless_pass_by_value)]\nfn get_system_status'''
if "async fn one_click_v2_build_plan" not in s:
    if anchor not in s:
        raise SystemExit("lib command anchor missing")
    s = s.replace(anchor, commands, 1)

old = '''struct InitializedApplication {\n    scanner: Arc<ScannerApplicationService>,\n    execution: Arc<ExecutionApplicationService>,\n    embedding_provider: Arc<OnnxLocalEmbeddingProvider>,\n}'''
new = '''struct InitializedApplication {\n    scanner: Arc<ScannerApplicationService>,\n    execution: Arc<ExecutionApplicationService>,\n    embedding_provider: Arc<OnnxLocalEmbeddingProvider>,\n    one_click_v2: one_click_v2::OneClickV2State,\n}'''
if "one_click_v2: one_click_v2::OneClickV2State" not in s:
    if old not in s:
        raise SystemExit("InitializedApplication anchor missing")
    s = s.replace(old, new, 1)

old = "    let mut executor_root_authority = load_or_create_executor_root(&secret_store)?;\n"
new = old + '    let one_click_v2_key = blake3::derive_key("zemo.one-click-v2.undo-journal.v1", &executor_root_authority);\n'
if "zemo.one-click-v2.undo-journal.v1" not in s:
    if old not in s:
        raise SystemExit("executor root anchor missing")
    s = s.replace(old, new, 1)

old = '''    Ok(InitializedApplication {\n        scanner,\n        execution,\n        embedding_provider,\n    })'''
new = '''    let one_click_v2 = one_click_v2::OneClickV2State::new(\n        data_dir.join("one-click-v2-undo.json"),\n        one_click_v2_key,\n    );\n    Ok(InitializedApplication {\n        scanner,\n        execution,\n        embedding_provider,\n        one_click_v2,\n    })'''
if 'data_dir.join("one-click-v2-undo.json")' not in s:
    if old not in s:
        raise SystemExit("InitializedApplication return anchor missing")
    s = s.replace(old, new, 1)

old = '''            app.manage(ManagedScanner {\n                service: services.scanner,'''
new = '''            app.manage(services.one_click_v2.clone());\n            app.manage(ManagedScanner {\n                service: services.scanner,'''
if "app.manage(services.one_click_v2.clone());" not in s:
    if old not in s:
        raise SystemExit("app.manage anchor missing")
    s = s.replace(old, new, 1)

old = '''            get_system_status,\n            restore_workspace_session,'''
new = '''            get_system_status,\n            one_click_v2_build_plan,\n            one_click_v2_apply,\n            one_click_v2_undo,\n            restore_workspace_session,'''
if "            one_click_v2_build_plan," not in s:
    if old not in s:
        raise SystemExit("handler anchor missing")
    s = s.replace(old, new, 1)
lib.write_text(s)

# Frontend contracts.
types = Path("apps/desktop/src/types.ts")
s = types.read_text()
if "export type OneClickV2Plan" not in s:
    s += '''\n\nexport type OneClickV2Move = {\n  source: string;\n  destination: string;\n  category: string;\n  reason: string;\n};\n\nexport type OneClickV2RootResult = {\n  kind: string;\n  displayLabel: string;\n  root: string;\n  filesSeen: number;\n  proposedMoves: OneClickV2Move[];\n  skipped: number;\n  errors: string[];\n};\n\nexport type OneClickV2Plan = {\n  planId: string;\n  roots: OneClickV2RootResult[];\n  filesSeen: number;\n  proposedMoves: number;\n};\n\nexport type OneClickV2ApplyResult = {\n  applied: Array<{ source: string; destination: string }>;\n  skipped: number;\n  errors: string[];\n};\n'''
    types.write_text(s)

api = Path("apps/desktop/src/api.ts")
s = api.read_text()
if "OneClickV2Plan," not in s:
    s = s.replace(
        "  OrganizationPreferences,\n",
        "  OrganizationPreferences,\n  OneClickV2ApplyResult,\n  OneClickV2Plan,\n",
        1,
    )
if "export function buildOneClickV2Plan" not in s:
    anchor = "export function getSystemStatus(): Promise<SystemStatus> {"
    block = '''export function buildOneClickV2Plan(): Promise<OneClickV2Plan> {\n  return invoke<OneClickV2Plan>("one_click_v2_build_plan");\n}\n\nexport function applyOneClickV2(): Promise<OneClickV2ApplyResult> {\n  return invoke<OneClickV2ApplyResult>("one_click_v2_apply");\n}\n\nexport function undoOneClickV2(): Promise<OneClickV2ApplyResult> {\n  return invoke<OneClickV2ApplyResult>("one_click_v2_undo");\n}\n\n'''
    if anchor not in s:
        raise SystemExit("api anchor missing")
    s = s.replace(anchor, block + anchor, 1)
api.write_text(s)

summary = Path("apps/desktop/src/oneClickSummary.ts")
s = summary.read_text()
if "OneClickV2Plan" not in s:
    s = s.replace(
        'import type { OrganizationOperation, OrganizationProposal } from "./types";',
        'import type { OneClickV2Plan, OrganizationOperation, OrganizationProposal } from "./types";',
        1,
    )
if "export function summarizeV2Plan" not in s:
    s += '''\n\nexport function summarizeV2Plan(\n  plan: OneClickV2Plan | null,\n): { filesToOrganize: number; counts: CategoryCounts } {\n  const counts = emptyCategoryCounts();\n  if (!plan) return { filesToOrganize: 0, counts };\n  for (const root of plan.roots) {\n    for (const move of root.proposedMoves) {\n      const head = move.category.split("/")[0];\n      if (head === "Documents") counts.Documents += 1;\n      else if (head === "Images") counts.Images += 1;\n      else if (head === "Vidéos" || head === "Videos") counts.Vidéos += 1;\n      else if (head === "Archives") counts.Archives += 1;\n      else if (head === "Installateurs") counts.Installateurs += 1;\n      else counts["À vérifier"] += 1;\n    }\n  }\n  return { filesToOrganize: plan.proposedMoves, counts };\n}\n'''
summary.write_text(s)

preview = Path("apps/desktop/src/OneClickOrganize.tsx")
s = preview.read_text()
if "OneClickV2Move" not in s:
    s = s.replace(
        'import type { FolderAccessProbe, RegisterUserContentRootResult } from "./types";',
        'import type { FolderAccessProbe, OneClickV2Move, RegisterUserContentRootResult } from "./types";',
        1,
    )
old = '''  onChooseAnother?: () => void;\n};'''
new = '''  onChooseAnother?: () => void;\n  moves?: OneClickV2Move[];\n};'''
if "moves?: OneClickV2Move[];" not in s:
    if old not in s:
        raise SystemExit("preview props anchor missing")
    s = s.replace(old, new, 1)
old = '''  onChooseAnother,\n}: OneClickPreviewViewProps) {'''
new = '''  onChooseAnother,\n  moves = [],\n}: OneClickPreviewViewProps) {'''
if "  moves = []," not in s:
    if old not in s:
        raise SystemExit("preview destructure anchor missing")
    s = s.replace(old, new, 1)
old = '''      <ul className="one-click-category-list">\n        {PREVIEW_CATEGORY_ORDER.map((category) => (\n          <li key={category}>\n            <span>{category}</span>\n            <strong>{counts[category].toLocaleString()}</strong>\n          </li>\n        ))}\n      </ul>'''
new = old + '''\n      {moves.length > 0 ? (\n        <details className="one-click-technical" open>\n          <summary>Fichiers qui seront déplacés ({moves.length.toLocaleString()})</summary>\n          <ul className="one-click-move-list">\n            {moves.slice(0, 250).map((move) => (\n              <li key={`${move.source}=>${move.destination}`}>\n                <code>{move.source}</code>\n                <span aria-hidden="true">→</span>\n                <code>{move.destination}</code>\n              </li>\n            ))}\n          </ul>\n          {moves.length > 250 ? <p>Seuls les 250 premiers déplacements sont affichés.</p> : null}\n        </details>\n      ) : null}'''
if 'className="one-click-move-list"' not in s:
    if old not in s:
        raise SystemExit("preview list anchor missing")
    s = s.replace(old, new, 1)
preview.write_text(s)

app = Path("apps/desktop/src/App.tsx")
s = app.read_text()
for name in [
    "  approveExecution,\n",
    "  prepareExecution,\n",
    "  rollbackExecution,\n",
    "  setOrganizationProposalStatus,\n",
    "  startExecution,\n",
    "  registerUserContentRoot,\n",
]:
    s = s.replace(name, "")
if "  applyOneClickV2," not in s:
    s = s.replace(
        "  analyzeSemantics,\n",
        "  analyzeSemantics,\n  applyOneClickV2,\n  buildOneClickV2Plan,\n",
        1,
    )
    s = s.replace(
        "  subscribeSemanticAnalysisProgress,\n",
        "  subscribeSemanticAnalysisProgress,\n  undoOneClickV2,\n",
        1,
    )
if "OneClickV2Plan," not in s:
    s = s.replace("  MonitoringDashboard,\n", "  MonitoringDashboard,\n  OneClickV2Plan,\n", 1)
s = s.replace("  OrganizationProposal,\n", "")
s = s.replace('import { summarizeProposals } from "./oneClickSummary";', 'import { summarizeV2Plan } from "./oneClickSummary";')

old = '''  const [oneClickProposals, setOneClickProposals] = useState<\n    OrganizationProposal[]\n  >([]);'''
new = '''  const [oneClickV2Plan, setOneClickV2Plan] = useState<OneClickV2Plan | null>(null);'''
if old in s:
    s = s.replace(old, new, 1)
s = s.replace("    setOneClickProposals([]);\n", "    setOneClickV2Plan(null);\n")

start = s.index("  async function scanAccessibleFolders(")
end = s.index("\n  async function handleAuthorizeFolder", start)
replacement = '''  async function scanAccessibleFolders(\n    _workspaceId: string,\n    working: FolderAccessProbe[],\n    _accessible: FolderAccessProbe[],\n  ) {\n    const plan = await buildOneClickV2Plan();\n    setOneClickV2Plan(plan);\n    setOneClickFilesAnalyzed(plan.filesSeen);\n    setOneClickFolders((current) =>\n      current.map((folder) => {\n        const probe = working.find((item) => item.kind === folder.kind);\n        const planned = plan.roots.find(\n          (item) => item.kind === folder.kind || item.root === probe?.resolvedPath,\n        );\n        if (!planned) return folder;\n        return {\n          ...folder,\n          phase: planned.errors.length > 0 ? "error" : "ready",\n          filesIndexed: planned.filesSeen,\n          humanStatus: planned.errors.length > 0 ? planned.errors.join(" · ") : undefined,\n        };\n      }),\n    );\n    const failures = plan.roots.flatMap((root) =>\n      root.errors.map((error) => `${root.displayLabel}: ${error}`),\n    );\n    if (plan.filesSeen === 0 && failures.length > 0) {\n      throw new Error(failures.join("\\n"));\n    }\n    setView("oneclick-preview");\n  }\n'''
s = s[:start] + replacement + s[end:]

start = s.index("  async function handleApplyOneClick()")
end = s.index("\n  async function handleCancel()", start)
replacement = '''  async function handleApplyOneClick() {\n    if (!oneClickV2Plan || oneClickV2Plan.proposedMoves === 0) return;\n    setApplyBusy(true);\n    clearError();\n    try {\n      const completed = await applyOneClickV2();\n      if (completed.errors.length > 0) {\n        throw new Error(completed.errors.join("\\n"));\n      }\n      const result = {\n        filesMoved: completed.applied.length,\n        executionIds: [],\n        completedAt: new Date().toISOString(),\n      };\n      writeLastOrganizeResult(result);\n      setLastOrganize(result);\n      setView("oneclick-done");\n    } catch (reason) {\n      reportError(reason, "organization");\n    } finally {\n      setApplyBusy(false);\n    }\n  }\n\n  async function handleUndoOneClick() {\n    if (!lastOrganize) return;\n    setUndoBusy(true);\n    clearError();\n    try {\n      const undone = await undoOneClickV2();\n      if (undone.errors.length > 0 || undone.skipped > 0) {\n        throw new Error(\n          [\n            ...undone.errors,\n            undone.skipped > 0\n              ? `${undone.skipped} fichier(s) n’ont pas pu être restaurés.`\n              : "",\n          ]\n            .filter(Boolean)\n            .join("\\n"),\n        );\n      }\n      clearLastOrganizeResult();\n      setLastOrganize(null);\n      setOneClickV2Plan(null);\n      setView("home");\n    } catch (reason) {\n      reportError(reason, "organization");\n    } finally {\n      setUndoBusy(false);\n    }\n  }\n'''
s = s[:start] + replacement + s[end:]

old = '''          filesToOrganize={summarizeProposals(oneClickProposals).filesToOrganize}\n          counts={summarizeProposals(oneClickProposals).counts}\n          applyBusy={applyBusy}\n          applyEnabled={Boolean(system?.applyEnabled) && !system?.journalLocked}\n          applyGateReason={system?.applyGateReason}'''
new = '''          filesToOrganize={summarizeV2Plan(oneClickV2Plan).filesToOrganize}\n          counts={summarizeV2Plan(oneClickV2Plan).counts}\n          moves={oneClickV2Plan?.roots.flatMap((root) => root.proposedMoves) ?? []}\n          applyBusy={applyBusy}\n          applyEnabled={Boolean(oneClickV2Plan && oneClickV2Plan.proposedMoves > 0)}\n          applyGateReason={oneClickV2Plan?.proposedMoves ? null : "Aucun fichier personnel à ranger."}'''
if old not in s:
    raise SystemExit("App preview anchor missing")
s = s.replace(old, new, 1)
app.write_text(s)

print("one-click v2 wiring applied")
