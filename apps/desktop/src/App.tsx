import { useEffect, useMemo, useState } from "react";
import {
  analyzeContent,
  analyzeSemantics,
  cancelContentAnalysis,
  cancelScan,
  cancelSemanticAnalysis,
  createWorkspace,
  getMonitoringDashboard,
  getSystemStatus,
  listScanDuplicates,
  listScanErrors,
  listScanFiles,
  listContentResults,
  restoreWorkspaceSession,
  registerUserContentRoot,
  scanWorkspace,
  selectAndRegisterRoot,
  subscribeScanProgress,
  subscribeContentAnalysisProgress,
  subscribeSemanticAnalysisProgress,
} from "./api";
import type {
  ContentAnalysis,
  ContentAnalysisProgress,
  ContentDetail,
  DuplicateGroup,
  InventorySort,
  MonitoringDashboard,
  RegisterUserContentRootResult,
  RegisteredRoot,
  ScanFile,
  ScanIssue,
  ScanProgress,
  ScanResult,
  SemanticAnalysis,
  SemanticAnalysisProgress,
  SystemStatus,
  Workspace,
} from "./types";
import { FileDetailPanel } from "./FileDetailPanel";
import {
  HomeDashboard,
  type AppDestination,
  type PrimaryAction,
} from "./HomeDashboard";
import { classifyUserError, shouldShowGlobalBanner, type UserFacingError } from "./errors";
import { IdentityDetailPanel } from "./IdentityDetailPanel";
import { IdentityReviewView } from "./IdentityReviewView";
import { MonitoringView } from "./MonitoringView";
import { OnboardingView } from "./OnboardingView";
import { OrganizationPreviewView } from "./OrganizationPreviewView";
import { ReviewView } from "./ReviewView";
import { RulesPreferencesView } from "./RulesPreferencesView";
import { SearchView } from "./SearchView";
import {
  isOnboardingCompleted,
  markOnboardingCompleted,
} from "./onboardingStorage";
import "./App.css";

type ResultView =
  | "home"
  | "summary"
  | "files"
  | "duplicates"
  | "errors"
  | "content"
  | "search"
  | "review"
  | "relationships"
  | "monitoring"
  | "rules"
  | "organization";

const EMPTY_PROGRESS: Omit<ScanProgress, "scanId"> = {
  phase: "DISCOVERING",
  filesDiscovered: 0,
  filesIndexed: 0,
  directoriesDiscovered: 0,
  bytesDiscovered: 0,
  filesHashed: 0,
  duplicateGroups: 0,
  errors: 0,
  skippedItems: 0,
};

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) {
    return "0 B";
  }
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  const index = Math.min(
    Math.floor(Math.log(value) / Math.log(1024)),
    units.length - 1,
  );
  return `${(value / 1024 ** index).toFixed(index === 0 ? 0 : 1)} ${units[index]}`;
}

function formatNativeTimestamp(value?: string | null): string {
  if (!value) {
    return "—";
  }
  try {
    const milliseconds = Number(BigInt(value) / 1_000_000n);
    const date = new Date(milliseconds);
    return Number.isNaN(date.getTime()) ? "—" : date.toLocaleString();
  } catch {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? "—" : date.toLocaleString();
  }
}

function phaseLabel(phase: ScanProgress["phase"]): string {
  const labels: Record<ScanProgress["phase"], string> = {
    DISCOVERING: "Recherche de vos fichiers…",
    INSPECTING: "Analyse des documents…",
    HASHING: "Préparation de l’organisation…",
    PERSISTING: "Indexation pour la recherche…",
    COMPLETED: "Analyse terminée",
    CANCELLED: "Analyse annulée",
  };
  return labels[phase];
}

function navigateToView(
  destination: AppDestination,
): ResultView {
  switch (destination) {
    case "scan":
      return "summary";
    case "history":
      return "organization";
    default:
      return destination;
  }
}

function App() {
  const [system, setSystem] = useState<SystemStatus | null>(null);
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [root, setRoot] = useState<RegisteredRoot | null>(null);
  const [organizationRootId, setOrganizationRootId] = useState<string | null>(
    null,
  );
  const [scan, setScan] = useState<ScanResult | null>(null);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [analysis, setAnalysis] = useState<ContentAnalysis | null>(null);
  const [analysisProgress, setAnalysisProgress] =
    useState<ContentAnalysisProgress | null>(null);
  const [semanticAnalysis, setSemanticAnalysis] = useState<SemanticAnalysis | null>(null);
  const [semanticProgress, setSemanticProgress] =
    useState<SemanticAnalysisProgress | null>(null);
  const [contentResults, setContentResults] = useState<ContentDetail[]>([]);
  const [selectedContent, setSelectedContent] = useState<ContentDetail | null>(
    null,
  );
  const [detailFileId, setDetailFileId] = useState<string | null>(null);
  const [detailIdentityId, setDetailIdentityId] = useState<string | null>(null);
  const [view, setView] = useState<ResultView>("home");
  const [files, setFiles] = useState<ScanFile[]>([]);
  const [duplicates, setDuplicates] = useState<DuplicateGroup[]>([]);
  const [issues, setIssues] = useState<ScanIssue[]>([]);
  const [sortBy, setSortBy] = useState<InventorySort>("filename");
  const [descending, setDescending] = useState(false);
  const [busy, setBusy] = useState<
    | "select"
    | "scan"
    | "cancel"
    | "load"
    | "analysis"
    | "cancelAnalysis"
    | null
  >(null);
  const [error, setError] = useState<UserFacingError | null>(null);
  const [sessionRestoring, setSessionRestoring] = useState(true);
  const [monitoringDashboard, setMonitoringDashboard] =
    useState<MonitoringDashboard | null>(null);
  const [dashboardError, setDashboardError] = useState(false);
  const [dashboardRetryToken, setDashboardRetryToken] = useState(0);
  const [pendingSearchQuery, setPendingSearchQuery] = useState<string | null>(
    null,
  );
  const [showOnboarding, setShowOnboarding] = useState(
    () => !isOnboardingCompleted(),
  );
  const [wholeComputerBusy, setWholeComputerBusy] = useState(false);
  const [wholeComputerProgress, setWholeComputerProgress] = useState<string | null>(
    null,
  );
  const [accessSummary, setAccessSummary] = useState<
    RegisterUserContentRootResult[] | null
  >(null);

  function reportError(
    reason: unknown,
    preferredScope: UserFacingError["scope"] = "global",
  ) {
    setError(classifyUserError(reason, preferredScope));
  }

  function clearError() {
    setError(null);
  }

  useEffect(() => {
    let active = true;
    void getSystemStatus()
      .then((status) => {
        if (active) {
          setSystem(status);
        }
      })
      .catch((reason) => {
        if (active) {
          reportError(reason);
        }
      });
    void restoreWorkspaceSession()
      .then((session) => {
        if (!active) {
          return;
        }
        if (!session) {
          return;
        }
        setWorkspace(session.workspace);
        setRoot(session.root ?? null);
        setOrganizationRootId(session.root?.id ?? null);
        setScan(session.scan ?? null);
        setView("home");
      })
      .catch((reason) => {
        if (active) {
          reportError(reason);
        }
      })
      .finally(() => {
        if (active) {
          setSessionRestoring(false);
        }
      });
    let unsubscribe: (() => void) | undefined;
    void subscribeScanProgress((next) => {
      if (active) {
        setProgress(next);
      }
    })
      .then((stop) => {
        if (active) {
          unsubscribe = stop;
        } else {
          stop();
        }
      })
      .catch((reason) => {
        if (active) {
          reportError(reason);
        }
      });
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, []);

  useEffect(() => {
    let active = true;
    if (!workspace) {
      setMonitoringDashboard(null);
      setDashboardError(false);
      return;
    }
    setDashboardError(false);
    void getMonitoringDashboard(workspace.id)
      .then((dashboard) => {
        if (active) {
          setMonitoringDashboard(dashboard);
          setDashboardError(false);
        }
      })
      .catch(() => {
        if (active) {
          setMonitoringDashboard(null);
          setDashboardError(true);
        }
      });
    return () => {
      active = false;
    };
  }, [workspace, view, scan?.id, dashboardRetryToken]);

  useEffect(() => {
    let active = true;
    let unsubscribe: (() => void) | undefined;
    void subscribeContentAnalysisProgress((next) => {
      if (active) {
        setAnalysisProgress(next);
      }
    })
      .then((stop) => {
        if (active) {
          unsubscribe = stop;
        } else {
          stop();
        }
      })
      .catch((reason) => {
        if (active) {
          reportError(reason);
        }
      });
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, []);

  useEffect(() => {
    let active = true;
    let unsubscribe: (() => void) | undefined;
    void subscribeSemanticAnalysisProgress((next) => {
      if (active) {
        setSemanticProgress(next);
      }
    })
      .then((stop) => {
        if (active) {
          unsubscribe = stop;
        } else {
          stop();
        }
      })
      .catch((reason) => {
        if (active) {
          reportError(reason);
        }
      });
    return () => {
      active = false;
      unsubscribe?.();
    };
  }, []);

  useEffect(() => {
    if (!scan || view !== "files") {
      return;
    }
    setBusy("load");
    void listScanFiles(scan.id, sortBy, descending)
      .then(setFiles)
      .catch((reason) => reportError(reason))
      .finally(() => setBusy(null));
  }, [descending, scan, sortBy, view]);

  const visibleProgress = useMemo<ScanProgress | null>(() => {
    if (progress) {
      return progress;
    }
    return scan
      ? {
          scanId: scan.id,
          phase: scan.status === "CANCELLED" ? "CANCELLED" : "COMPLETED",
          filesDiscovered: scan.filesDiscovered,
          filesIndexed: scan.filesIndexed,
          directoriesDiscovered: scan.directoriesDiscovered,
          bytesDiscovered: scan.bytesDiscovered,
          filesHashed: scan.filesHashed,
          duplicateGroups: scan.duplicateGroups,
          errors: scan.errors,
          skippedItems: scan.skippedItems,
        }
      : null;
  }, [progress, scan]);

  function goTo(destination: AppDestination, options?: { searchQuery?: string }) {
    setDetailFileId(null);
    setDetailIdentityId(null);
    if (destination === "organization" || destination === "history") {
      setOrganizationRootId(root?.id ?? organizationRootId);
    }
    if (destination === "search") {
      setPendingSearchQuery(
        options && "searchQuery" in options ? (options.searchQuery ?? "") : null,
      );
    } else {
      setPendingSearchQuery(null);
    }
    setView(navigateToView(destination));
  }

  function handleSearchFromHome(query: string) {
    goTo("search", { searchQuery: query });
  }

  function handlePrimaryAction(action: PrimaryAction) {
    if (action.run === "selectFolder") {
      void handleSelectFolder();
      return;
    }
    if (action.run === "startScan") {
      void handleStartScan();
      return;
    }
    if (action.run === "analyze") {
      void handleAnalyzeContent();
      return;
    }
    if (action.destination) {
      goTo(action.destination);
    }
  }

  async function handleSelectFolder() {
    clearError();
        setBusy("select");
    try {
      let activeWorkspace = workspace;
      if (!activeWorkspace) {
        activeWorkspace = await createWorkspace("Inventaire local");
        setWorkspace(activeWorkspace);
      }
      const selected = await selectAndRegisterRoot(activeWorkspace.id);
      setRoot(selected);
      setOrganizationRootId(selected.id);
      setScan(null);
      setProgress(null);
      setAnalysis(null);
      setAnalysisProgress(null);
      setSemanticAnalysis(null);
      setSemanticProgress(null);
      setContentResults([]);
      setSelectedContent(null);
      setDetailFileId(null);
      setDetailIdentityId(null);
      setFiles([]);
      setDuplicates([]);
      setIssues([]);
      setView("home");
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(null);
    }
  }

  async function handleStartScan() {
    if (!workspace || !root) {
      return;
    }
    clearError();
    setScan(null);
    setAnalysis(null);
    setAnalysisProgress(null);
    setSemanticAnalysis(null);
    setSemanticProgress(null);
    setContentResults([]);
    setSelectedContent(null);
    setDetailFileId(null);
    setDetailIdentityId(null);
    setProgress({
      scanId: "",
      ...EMPTY_PROGRESS,
    });
    setView("summary");
    setBusy("scan");
    try {
      const result = await scanWorkspace(workspace.id);
      setScan(result);
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(null);
    }
  }

  async function handleWholeComputer(kinds: string[]) {
    clearError();
        setAccessSummary(null);
    setWholeComputerBusy(true);
    setWholeComputerProgress("Préparation…");
    try {
      let activeWorkspace = workspace;
      if (!activeWorkspace) {
        activeWorkspace = await createWorkspace("Inventaire local");
        setWorkspace(activeWorkspace);
      }
      const outcomes: RegisterUserContentRootResult[] = [];
      let lastRoot: RegisteredRoot | null = null;
      let lastScan: ScanResult | null = null;
      let index = 0;
      for (const kind of kinds) {
        index += 1;
        setWholeComputerProgress(
          `Accès aux dossiers (${index}/${kinds.length})…`,
        );
        const outcome = await registerUserContentRoot(activeWorkspace.id, kind);
        outcomes.push(outcome);
        if (!outcome.root) {
          continue;
        }
        lastRoot = outcome.root;
        setRoot(outcome.root);
        setOrganizationRootId(outcome.root.id);
        setWholeComputerProgress(
          `Analyse de ${outcome.displayLabel} (${index}/${kinds.length})…`,
        );
        setBusy("scan");
        const result = await scanWorkspace(activeWorkspace.id);
        lastScan = result;
        setScan(result);
      }
      setAccessSummary(outcomes);
      if (lastRoot) {
        setRoot(lastRoot);
        setOrganizationRootId(lastRoot.id);
      }
      if (lastScan) {
        setScan(lastScan);
      }
      markOnboardingCompleted();
      setShowOnboarding(false);
      setView("home");
      if (!outcomes.some((item) => item.root)) {
        setError({
          title: "Aucun dossier analysé",
          message:
            "Aucun dossier n’a pu être analysé. Vérifiez les accès ou choisissez un dossier manuellement.",
          impact: "Vous pouvez réessayer ou choisir un dossier manuellement.",
          actionHint: "Choisissez un dossier ou réessayez Organiser mon ordinateur.",
          severity: "action_required",
          scope: "permission",
          technicalDetails: null,
        });
      } else if (outcomes.some((item) => item.status === "denied")) {
        setError({
          title: "Accès partiel",
          message:
            "Certains dossiers n’ont pas pu être analysés faute d’accès.",
          impact: "Les dossiers disponibles ont été analysés normalement.",
          actionHint: "Réessayez plus tard ou continuez avec les dossiers disponibles.",
          severity: "action_required",
          scope: "permission",
          technicalDetails: null,
        });
      }
    } catch (reason) {
      reportError(reason);
    } finally {
      setWholeComputerBusy(false);
      setWholeComputerProgress(null);
      setBusy(null);
    }
  }

  async function handleCancel() {
    if (!workspace) {
      return;
    }
    setBusy("cancel");
    try {
      await cancelScan(workspace.id);
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy("scan");
    }
  }

  async function handleAnalyzeContent() {
    if (!scan) {
      return;
    }
    clearError();
    setAnalysis(null);
    setSemanticAnalysis(null);
    setSemanticProgress(null);
    setAnalysisProgress({
      batchId: "",
      scanId: scan.id,
      phase: "RUNNING",
      filesQueued: scan.filesIndexed,
      filesCompleted: 0,
      successful: 0,
      partial: 0,
      unsupported: 0,
      skipped: 0,
      failed: 0,
      ocrProcessed: 0,
    });
    setContentResults([]);
    setSelectedContent(null);
    setDetailFileId(null);
    setDetailIdentityId(null);
    setView("content");
    setBusy("analysis");
    try {
      const result = await analyzeContent(scan.id);
      setAnalysis(result);
      const details = await listContentResults(result.id);
      setContentResults(details);
      setSelectedContent(details[0] ?? null);
      if (["CANCELLED", "FAILED"].includes(result.status.toUpperCase())) {
        return;
      }
      setSemanticProgress({
        batchId: "",
        scanId: scan.id,
        phase: "RUNNING",
        filesQueued: result.filesCompleted,
        filesCompleted: 0,
        highConfidence: 0,
        needsReview: 0,
        unknown: 0,
        partial: 0,
        failed: 0,
      });
      setSemanticAnalysis(await analyzeSemantics(scan.id));
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(null);
    }
  }

  async function handleCancelAnalysis() {
    if (!scan) {
      return;
    }
    setBusy("cancelAnalysis");
    try {
      await Promise.allSettled([
        cancelContentAnalysis(scan.id),
        cancelSemanticAnalysis(scan.id),
      ]);
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy("analysis");
    }
  }

  async function showDuplicates() {
    if (!scan) {
      return;
    }
    setDetailFileId(null);
    setDetailIdentityId(null);
    setView("duplicates");
    setBusy("load");
    try {
      setDuplicates(await listScanDuplicates(scan.id));
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(null);
    }
  }

  async function showErrors() {
    if (!scan) {
      return;
    }
    setDetailFileId(null);
    setDetailIdentityId(null);
    setView("errors");
    setBusy("load");
    try {
      setIssues(await listScanErrors(scan.id));
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(null);
    }
  }

  function changeSort(next: InventorySort) {
    if (sortBy === next) {
      setDescending((current) => !current);
    } else {
      setSortBy(next);
      setDescending(false);
    }
  }

  const scanRunning = busy === "scan" || busy === "cancel";
  const contentRunning = busy === "analysis" || busy === "cancelAnalysis";
  const completed = scan !== null;

  return (
    <main className="scanner-shell">
      {showOnboarding ? (
        <OnboardingView
          selectedPath={root?.selectedPath ?? null}
          selectBusy={busy === "select"}
          wholeComputerBusy={wholeComputerBusy}
          onSelectFolder={handleSelectFolder}
          onStartWholeComputer={handleWholeComputer}
          onComplete={() => {
            markOnboardingCompleted();
            setShowOnboarding(false);
          }}
        />
      ) : null}

      <header className="scanner-header">
        <div>
          <span className="eyebrow">
            ZEMO · Bêta privée macOS · {system?.version ?? "0.1.0"}-beta.5
          </span>
          <h1>Organisez et retrouvez vos fichiers.</h1>
          <p>
            Analyse locale. Organisation proposée. Recherche simple. Les
            fichiers ne sont déplacés que si vous appliquez l’organisation,
            après confirmation.
          </p>
        </div>
        <div className="scanner-header-aside">
          <button
            type="button"
            className="help-tour-button"
            onClick={() => setShowOnboarding(true)}
            aria-label="Ouvrir la visite guidée"
          >
            Aide / Visite
          </button>
          <div className="safety-state" aria-label="Garanties du scanner">
            <span>
              {system?.localFirst ? "Traitement local" : "Vérification…"}
            </span>
            <span>
              {system?.readOnlyScan ? "Rien n’est déplacé au scan" : "Vérification…"}
            </span>
            <span>
              {system?.networkDisabled ? "Aucun upload" : "Vérification…"}
            </span>
          </div>
        </div>
      </header>

      <nav className="app-nav" aria-label="Navigation principale">
        {(
          [
            ["home", "Accueil"],
            ["organization", "Organisation"],
            ["search", "Recherche"],
            ["monitoring", "Surveillance"],
          ] as const
        ).map(([destination, label]) => {
          const activeView = navigateToView(destination);
          const isActive = view === activeView;
          return (
            <button
              key={destination}
              type="button"
              className={isActive ? "app-nav__item app-nav__item--active" : "app-nav__item"}
              aria-current={isActive ? "page" : undefined}
              disabled={destination !== "home" && !workspace}
              onClick={() => goTo(destination)}
            >
              {label}
            </button>
          );
        })}
        <details className="app-nav-advanced">
          <summary>Options avancées</summary>
          <div className="app-nav-advanced__items">
            <button
              type="button"
              className={
                view === "review"
                  ? "app-nav__item app-nav__item--active"
                  : "app-nav__item"
              }
              disabled={!workspace}
              onClick={() => goTo("review")}
            >
              À revoir
            </button>
            <button
              type="button"
              className={
                view === "rules"
                  ? "app-nav__item app-nav__item--active"
                  : "app-nav__item"
              }
              disabled={!workspace}
              onClick={() => goTo("rules")}
            >
              Préférences de rangement
            </button>
            <button
              type="button"
              className={
                ["summary", "files", "duplicates", "errors", "content"].includes(
                  view,
                )
                  ? "app-nav__item app-nav__item--active"
                  : "app-nav__item"
              }
              onClick={() => goTo("scan")}
            >
              Inventaire
            </button>
            <button
              type="button"
              className={
                view === "organization" && (system?.recoveryRequired || system?.journalLocked)
                  ? "app-nav__item app-nav__item--active"
                  : "app-nav__item"
              }
              disabled={!workspace}
              onClick={() => goTo("history")}
            >
              {system?.recoveryRequired || system?.journalLocked
                ? "Récupération requise"
                : "Historique d’exécution"}
            </button>
          </div>
        </details>
      </nav>

      {system?.recoveryRequired || system?.journalLocked ? (
        <div className="notice-banner notice-banner--critical" role="alert">
          <div>
            <strong>Attention requise</strong>
            <span>
              Une opération précédente nécessite votre attention. Les
              modifications de fichiers restent bloquées.
            </span>
          </div>
          <button type="button" onClick={() => goTo("history")}>
            Examiner
          </button>
        </div>
      ) : null}

      {wholeComputerProgress ? (
        <div className="whole-computer-progress" role="status" aria-live="polite">
          <strong>Analyse en cours</strong>
          <span>{wholeComputerProgress}</span>
        </div>
      ) : null}

      {accessSummary && accessSummary.some((item) => item.status !== "registered") ? (
        <div className="access-summary" role="status">
          <strong>Accès aux dossiers</strong>
          <ul>
            {accessSummary.map((item) => (
              <li key={item.kind}>
                {item.displayLabel}{" "}
                {item.status === "registered"
                  ? "✓"
                  : item.status === "denied"
                    ? "— accès refusé"
                    : `— ${item.status}`}
              </li>
            ))}
          </ul>
          <button type="button" onClick={() => setAccessSummary(null)}>
            Fermer
          </button>
        </div>
      ) : null}

      {error && shouldShowGlobalBanner(error) ? (
        <div
          className={`notice-banner notice-banner--${error.severity}`}
          role={error.severity === "critical" ? "alert" : "status"}
        >
          <div>
            <strong>{error.title}</strong>
            <span>{error.message}</span>
            <span className="notice-banner__impact">{error.impact}</span>
            <span className="notice-banner__hint">{error.actionHint}</span>
            {error.technicalDetails ? (
              <details className="error-details">
                <summary>Afficher les détails</summary>
                <code>{error.technicalDetails}</code>
              </details>
            ) : null}
          </div>
          <button type="button" onClick={clearError}>
            Fermer
          </button>
        </div>
      ) : null}

      {view === "home" ? (
        <HomeDashboard
          loading={sessionRestoring}
          system={system}
          workspaceId={workspace?.id ?? null}
          root={root}
          scan={scan}
          dashboard={monitoringDashboard}
          dashboardError={dashboardError}
          contentNeedsReview={semanticAnalysis?.needsReview ?? null}
          contentFailed={analysis?.failed ?? null}
          contentUnsupported={analysis?.unsupported ?? null}
          onPrimaryAction={handlePrimaryAction}
          onNavigate={goTo}
          onSearch={handleSearchFromHome}
          onRetryDashboard={() => {
            setDashboardRetryToken((value) => value + 1);
          }}
        />
      ) : null}

      {view !== "home" &&
      ["summary", "files", "duplicates", "errors", "content", "scan"].includes(
        view,
      ) ? (
      <section className="scan-controls" aria-labelledby="scan-scope-title">
        <div>
          <span className="step">01</span>
          <h2 id="scan-scope-title">Dossier à analyser</h2>
          <p>
            Aucun dossier n’est choisi automatiquement. Vous contrôlez toujours
            le périmètre.
          </p>
        </div>
        <div className="scope-actions">
          <button
            className="primary"
            type="button"
            disabled={busy !== null}
            onClick={handleSelectFolder}
          >
            {busy === "select" ? "Sélection…" : "Choisir un dossier"}
          </button>
          <button
            type="button"
            disabled={!root || busy !== null}
            onClick={handleStartScan}
          >
            {busy === "scan" ? "Scan…" : "Scanner"}
          </button>
          {scanRunning ? (
            <button className="danger-outline" type="button" onClick={handleCancel}>
              Annuler
            </button>
          ) : null}
        </div>
        <div className="selected-path">
          <span>Dossier sélectionné</span>
          <code>{root?.selectedPath ?? "Aucun dossier sélectionné"}</code>
        </div>
      </section>
      ) : null}

      {workspace && view === "monitoring" ? (
        <MonitoringView
          workspaceId={workspace.id}
          onOpenReview={() => setView("review")}
          onOpenOrganization={(rootId) => {
            setOrganizationRootId(rootId);
            setView("organization");
          }}
        />
      ) : null}

      {workspace && view === "rules" ? (
        <RulesPreferencesView workspaceId={workspace.id} />
      ) : null}

      {visibleProgress &&
      !["monitoring", "rules", "home"].includes(view) &&
      (scanRunning ||
        (progress !== null &&
          !["COMPLETED", "CANCELLED"].includes(progress.phase))) ? (
        <section className="progress-panel" aria-live="polite">
          <div className="progress-title">
            <div>
              <span className="step">02</span>
              <h2>{phaseLabel(visibleProgress.phase)}</h2>
            </div>
            <span className={`status status--${visibleProgress.phase.toLowerCase()}`}>
              {scanRunning
                ? "Scan en cours"
                : scan?.status === "COMPLETED_WITH_ERRORS"
                  ? "Terminé avec alertes"
                  : scan?.status === "COMPLETED"
                    ? "Terminé"
                    : scan?.status ?? visibleProgress.phase}
            </span>
          </div>
          <div className="metric-grid">
            <Metric label="Fichiers trouvés" value={visibleProgress.filesDiscovered} />
            <Metric label="Fichiers indexés" value={visibleProgress.filesIndexed} />
            <Metric
              label="Dossiers"
              value={visibleProgress.directoriesDiscovered}
            />
            <Metric
              label="Taille totale"
              value={formatBytes(visibleProgress.bytesDiscovered)}
            />
            <Metric label="Inventaire préparé" value={visibleProgress.filesHashed} />
            <Metric label="Erreurs" value={visibleProgress.errors} />
          </div>
        </section>
      ) : null}

      {analysisProgress && view !== "home" ? (
        <section className="progress-panel content-progress" aria-live="polite">
          <div className="progress-title">
            <div>
              <span className="step">04</span>
              <h2>Lecture des documents</h2>
            </div>
            <div className="progress-actions">
              <span className={`status status--${analysisProgress.phase.toLowerCase()}`}>
                {analysisProgress.phase === "RUNNING"
                  ? "En cours"
                  : analysisProgress.phase}
              </span>
              {contentRunning ? (
                <button
                  className="danger-outline"
                  type="button"
                  onClick={handleCancelAnalysis}
                >
                  Arrêter
                </button>
              ) : null}
            </div>
          </div>
          <div className="metric-grid content-metrics">
            <Metric label="En file" value={analysisProgress.filesQueued} />
            <Metric label="Terminés" value={analysisProgress.filesCompleted} />
            <Metric label="Réussis" value={analysisProgress.successful} />
            <Metric label="Partiels" value={analysisProgress.partial} />
            <Metric label="Non pris en charge" value={analysisProgress.unsupported} />
            <Metric label="Échoués" value={analysisProgress.failed} />
            <Metric label="Ignorés" value={analysisProgress.skipped} />
            <Metric label="Texte relu (OCR)" value={analysisProgress.ocrProcessed} />
          </div>
        </section>
      ) : null}

      {semanticProgress && view !== "home" ? (
        <section className="progress-panel semantic-progress" aria-live="polite">
          <div className="progress-title">
            <div>
              <span className="step">05</span>
              <h2>Compréhension du contenu</h2>
            </div>
            <div className="progress-actions">
              <span className={`status status--${semanticProgress.phase.toLowerCase()}`}>
                {semanticProgress.phase === "RUNNING"
                  ? "En cours"
                  : semanticAnalysis?.status ?? semanticProgress.phase}
              </span>
              {semanticProgress.phase === "RUNNING" && contentRunning ? (
                <button
                  className="danger-outline"
                  type="button"
                  onClick={handleCancelAnalysis}
                >
                  Arrêter
                </button>
              ) : null}
            </div>
          </div>
          <div className="metric-grid content-metrics">
            <Metric label="Compris" value={semanticProgress.filesCompleted} />
            <Metric label="En file" value={semanticProgress.filesQueued} />
            <Metric label="Clairs" value={semanticProgress.highConfidence} />
            <Metric label="À revoir" value={semanticProgress.needsReview} />
            <Metric label="Inconnus" value={semanticProgress.unknown} />
            <Metric label="Partiels" value={semanticProgress.partial} />
            <Metric label="Échoués" value={semanticProgress.failed} />
          </div>
        </section>
      ) : null}

      {workspace &&
      ["search", "review", "relationships", "organization"].includes(view) ? (
        <section className="results-panel" aria-label="Résultats de l’espace de travail">
          <nav className="result-tabs" aria-label="Vues des résultats">
            {scan ? (
              <button
                className="primary"
                type="button"
                disabled={
                  busy !== null ||
                  !["COMPLETED", "COMPLETED_WITH_ERRORS"].includes(scan.status)
                }
                onClick={handleAnalyzeContent}
              >
                {contentRunning ? "Analyse…" : "Analyser les documents"}
              </button>
            ) : null}
            <button type="button" onClick={() => setView("search")}>
              Recherche
            </button>
            <button type="button" onClick={() => setView("review")}>
              À revoir
            </button>
            <button type="button" onClick={() => setView("relationships")}>
              Relations
            </button>
            <button
              type="button"
              onClick={() => {
                setOrganizationRootId(root?.id ?? organizationRootId);
                setView("organization");
              }}
            >
              Organisation
            </button>
          </nav>
          {detailFileId ? (
            <FileDetailPanel
              fileId={detailFileId}
              onClose={() => setDetailFileId(null)}
              onOpenIdentity={(identityId) => {
                setDetailFileId(null);
                setDetailIdentityId(identityId);
              }}
            />
          ) : null}
          {detailIdentityId ? (
            <IdentityDetailPanel
              identityId={detailIdentityId}
              onClose={() => setDetailIdentityId(null)}
              onOpenFile={(fileId) => {
                setDetailIdentityId(null);
                setDetailFileId(fileId);
              }}
              onOpenIdentity={(identityId) => {
                setDetailFileId(null);
                setDetailIdentityId(identityId);
              }}
            />
          ) : null}
          {view === "search" ? (
            <SearchView
              workspaceId={workspace.id}
              initialQuery={pendingSearchQuery ?? undefined}
              onOpenFile={setDetailFileId}
            />
          ) : null}
          {view === "review" ? (
            <>
              <ReviewView
                workspaceId={workspace.id}
                onOpenFile={(fileId) => {
                  setDetailIdentityId(null);
                  setDetailFileId(fileId);
                }}
              />
              <IdentityReviewView
                workspaceId={workspace.id}
                onOpenIdentity={(identityId) => {
                  setDetailFileId(null);
                  setDetailIdentityId(identityId);
                }}
              />
            </>
          ) : null}
          {view === "relationships" ? (
            <IdentityReviewView
              workspaceId={workspace.id}
              onOpenIdentity={(identityId) => {
                setDetailFileId(null);
                setDetailIdentityId(identityId);
              }}
            />
          ) : null}
          {view === "organization" ? (
            <OrganizationPreviewView
              workspaceId={workspace.id}
              rootId={organizationRootId ?? root?.id}
            />
          ) : null}
        </section>
      ) : null}

      {completed &&
      ![
        "home",
        "monitoring",
        "rules",
        "search",
        "review",
        "relationships",
        "organization",
      ].includes(view) ? (
        <section className="results-panel" aria-labelledby="results-title">
          <div className="results-heading">
            <div>
              <span className="step">03</span>
              <h2 id="results-title">
                {scan.status === "CANCELLED" ? "Analyse annulée" : "Analyse terminée"}
              </h2>
              <p className="view-note">
                {scan.filesIndexed.toLocaleString()} fichiers analysés. Rien n’a
                encore été modifié sur votre ordinateur.
              </p>
            </div>
            {scan.truncated ? (
              <span className="warning-chip">Limite de sécurité atteinte</span>
            ) : null}
          </div>

          <div className="summary-list">
            <Summary label="Fichiers trouvés" value={scan.filesDiscovered} />
            <Summary label="Fichiers analysés" value={scan.filesIndexed} />
            <Summary label="Taille totale" value={formatBytes(scan.bytesDiscovered)} />
            <Summary label="Doublons" value={scan.duplicateGroups} />
            <Summary label="Erreurs" value={scan.errors} />
            <Summary label="Ignorés" value={scan.skippedItems} />
          </div>

          <nav className="result-tabs" aria-label="Prochaine étape">
            <button
              className="primary"
              type="button"
              disabled={
                !["COMPLETED", "COMPLETED_WITH_ERRORS"].includes(scan.status)
              }
              onClick={() => goTo("organization")}
            >
              Voir l’organisation
            </button>
            <button type="button" onClick={() => goTo("search")}>
              Rechercher un fichier
            </button>
            <button
              type="button"
              disabled={
                busy !== null ||
                !["COMPLETED", "COMPLETED_WITH_ERRORS"].includes(scan.status)
              }
              onClick={handleAnalyzeContent}
            >
              {contentRunning ? "Analyse…" : "Analyser les documents"}
            </button>
            <button type="button" onClick={() => goTo("review")}>
              À vérifier
            </button>
            <button
              type="button"
              onClick={() => {
                setDetailFileId(null);
                setDetailIdentityId(null);
                setView("relationships");
              }}
            >
              Relations
            </button>
            <button
              type="button"
              onClick={() => {
                setDetailFileId(null);
                setDetailIdentityId(null);
                setView("files");
              }}
            >
              Voir les fichiers
            </button>
            <button type="button" onClick={showDuplicates}>
              Voir les doublons
            </button>
            <button type="button" onClick={showErrors}>
              Voir les erreurs
            </button>
          </nav>

          {detailFileId ? (
            <FileDetailPanel
              fileId={detailFileId}
              onClose={() => setDetailFileId(null)}
              onOpenIdentity={(identityId) => {
                setDetailFileId(null);
                setDetailIdentityId(identityId);
              }}
            />
          ) : null}

          {detailIdentityId ? (
            <IdentityDetailPanel
              identityId={detailIdentityId}
              onClose={() => setDetailIdentityId(null)}
              onOpenFile={(fileId) => {
                setDetailIdentityId(null);
                setDetailFileId(fileId);
              }}
              onOpenIdentity={(identityId) => {
                setDetailFileId(null);
                setDetailIdentityId(identityId);
              }}
            />
          ) : null}

          {view === "search" && workspace ? (
            <SearchView
              workspaceId={workspace.id}
              initialQuery={pendingSearchQuery ?? undefined}
              onOpenFile={setDetailFileId}
            />
          ) : null}

          {view === "review" && workspace ? (
            <>
              <ReviewView
                workspaceId={workspace.id}
                onOpenFile={(fileId) => {
                  setDetailIdentityId(null);
                  setDetailFileId(fileId);
                }}
              />
              <IdentityReviewView
                workspaceId={workspace.id}
                onOpenIdentity={(identityId) => {
                  setDetailFileId(null);
                  setDetailIdentityId(identityId);
                }}
              />
            </>
          ) : null}

          {view === "relationships" && workspace ? (
            <IdentityReviewView
              workspaceId={workspace.id}
              onOpenIdentity={(identityId) => {
                setDetailFileId(null);
                setDetailIdentityId(identityId);
              }}
            />
          ) : null}

          {view === "organization" && workspace ? (
            <OrganizationPreviewView
              workspaceId={workspace.id}
              rootId={organizationRootId ?? root?.id}
            />
          ) : null}

          {view === "files" ? (
            <div className="table-wrap">
              <p className="view-note">
                Affichage limité aux 500 premiers fichiers selon le tri choisi.
              </p>
              <table>
                <thead>
                  <tr>
                    <Sortable label="Nom" onClick={() => changeSort("filename")} />
                    <Sortable label="Type" onClick={() => changeSort("type")} />
                    <Sortable label="Taille" onClick={() => changeSort("size")} />
                    <Sortable label="Modifié" onClick={() => changeSort("modified")} />
                    <Sortable
                      label="Emplacement"
                      onClick={() => changeSort("location")}
                    />
                    <Sortable label="État" onClick={() => changeSort("status")} />
                  </tr>
                </thead>
                <tbody>
                  {files.map((file) => (
                    <tr key={file.id}>
                      <td>
                        <button
                          type="button"
                          className="linkish"
                          onClick={() => setDetailFileId(file.id)}
                        >
                          {file.filename}
                        </button>
                      </td>
                      <td>{file.fileType ?? file.extension ?? "Inconnu"}</td>
                      <td>{formatBytes(file.byteSize)}</td>
                      <td>{formatNativeTimestamp(file.modifiedAt)}</td>
                      <td>
                        <code>{file.relativePath}</code>
                      </td>
                      <td>{file.status.replace(/_/g, " ")}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {!busy && files.length === 0 ? (
                <Empty text="Aucun fichier indexé." />
              ) : null}
            </div>
          ) : null}

          {view === "duplicates" ? (
            <div className="card-list">
              {duplicates.map((group) => (
                <article className="duplicate-card" key={group.digest}>
                  <div>
                    <strong>{group.files.length} identical files</strong>
                    <span>{formatBytes(group.byteSize)} each</span>
                  </div>
                  <ul>
                    {group.files.map((file) => (
                      <li key={file.id}>
                        <strong>{file.filename}</strong>
                        <code>{file.relativePath}</code>
                      </li>
                    ))}
                  </ul>
                  <small>BLAKE3 {group.digest.slice(0, 16)}…</small>
                </article>
              ))}
              {!busy && duplicates.length === 0 ? (
                <Empty text="No exact duplicate groups found." />
              ) : null}
            </div>
          ) : null}

          {view === "errors" ? (
            <div className="card-list">
              {issues.map((issue, index) => (
                <article className="issue-card" key={`${issue.relativePath}-${index}`}>
                  <span>{issue.category.replace(/_/g, " ")}</span>
                  <strong>{issue.relativePath || "Selected root"}</strong>
                  <p>{issue.message}</p>
                </article>
              ))}
              {!busy && issues.length === 0 ? (
                <Empty text="No scan issues recorded." />
              ) : null}
            </div>
          ) : null}

          {view === "content" ? (
            <div className="content-analysis-view">
              {analysis ? (
                <div className="summary-list content-summary">
                  <Summary label="Queued" value={analysis.filesQueued} />
                  <Summary label="Completed" value={analysis.filesCompleted} />
                  <Summary label="Successful" value={analysis.successful} />
                  <Summary label="Partial" value={analysis.partial} />
                  <Summary label="Unsupported" value={analysis.unsupported} />
                  <Summary label="Failed" value={analysis.failed} />
                  <Summary label="OCR" value={analysis.ocrProcessed} />
                </div>
              ) : null}
              {contentResults.length > 0 ? (
                <div className="content-layout">
                  <div className="content-file-list" aria-label="Analyzed files">
                    {contentResults.map((result) => (
                      <button
                        className={
                          selectedContent?.fileVersionId === result.fileVersionId
                            ? "content-file content-file--selected"
                            : "content-file"
                        }
                        type="button"
                        key={result.fileVersionId}
                        onClick={() => setSelectedContent(result)}
                      >
                        <strong>{result.filename}</strong>
                        <span>{result.status}</span>
                        <code>{result.relativePath}</code>
                      </button>
                    ))}
                  </div>
                  {selectedContent ? (
                    <article className="content-detail">
                      <div className="content-detail-heading">
                        <div>
                          <span className="step">FILE DETAIL</span>
                          <h2>{selectedContent.filename}</h2>
                        </div>
                        <span
                          className={`status status--${selectedContent.status.toLowerCase()}`}
                        >
                          {selectedContent.status}
                        </span>
                      </div>
                      <dl>
                        <div>
                          <dt>TYPE</dt>
                          <dd>
                            {selectedContent.detectedContentType ??
                              selectedContent.extension ??
                              "Unknown"}
                          </dd>
                        </div>
                        <div>
                          <dt>TEXT EXTRACTION</dt>
                          <dd>{selectedContent.status}</dd>
                        </div>
                        {selectedContent.pageCount != null ? (
                          <div>
                            <dt>PAGES</dt>
                            <dd>{selectedContent.pageCount}</dd>
                          </div>
                        ) : null}
                        {selectedContent.sheetCount != null ? (
                          <div>
                            <dt>SHEETS</dt>
                            <dd>{selectedContent.sheetCount}</dd>
                          </div>
                        ) : null}
                        {selectedContent.slideCount != null ? (
                          <div>
                            <dt>SLIDES</dt>
                            <dd>{selectedContent.slideCount}</dd>
                          </div>
                        ) : null}
                        {selectedContent.imageWidth && selectedContent.imageHeight ? (
                          <div>
                            <dt>DIMENSIONS</dt>
                            <dd>
                              {selectedContent.imageWidth} × {selectedContent.imageHeight}
                            </dd>
                          </div>
                        ) : null}
                        <div>
                          <dt>OCR</dt>
                          <dd>
                            {selectedContent.ocrUsed
                              ? "Local OCR used"
                              : selectedContent.requiresOcr
                                ? "Required but unavailable or incomplete"
                                : "Not required"}
                          </dd>
                        </div>
                        <div>
                          <dt>CHARACTERS</dt>
                          <dd>{selectedContent.characterCount}</dd>
                        </div>
                      </dl>
                      {selectedContent.typeMismatch ? (
                        <p className="detail-warning">TYPE_MISMATCH</p>
                      ) : null}
                      {selectedContent.errorCategory ? (
                        <p className="detail-warning">
                          {selectedContent.errorCategory}
                          {selectedContent.errorMessage
                            ? ` — ${selectedContent.errorMessage}`
                            : ""}
                        </p>
                      ) : null}
                      <h3>EXTRACTED TEXT PREVIEW</h3>
                      <pre className="text-preview">
                        {selectedContent.textPreview || "No text extracted."}
                      </pre>
                    </article>
                  ) : null}
                </div>
              ) : !contentRunning ? (
                <Empty text="No content analysis results available." />
              ) : (
                <p className="view-note">Bounded local extraction is running…</p>
              )}
            </div>
          ) : null}
        </section>
      ) : null}

      <footer>
        Aucun fichier source n’est modifié et aucun contenu extrait ne quitte cet
        appareil.
      </footer>
    </main>
  );
}

function Metric({ label, value }: { label: string; value: number | string }) {
  return (
    <div className="metric">
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function Summary({ label, value }: { label: string; value: number | string }) {
  return (
    <div>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}

function Sortable({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <th>
      <button type="button" onClick={onClick}>
        {label}
      </button>
    </th>
  );
}

function Empty({ text }: { text: string }) {
  return <p className="empty-state">{text}</p>;
}

export default App;
