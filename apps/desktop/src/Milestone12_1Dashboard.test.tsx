// @vitest-environment jsdom

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import App from "./App";
import {
  HomeDashboard,
  MAX_RECENT_ACTIVITY,
  formatProposedDestination,
  resolveOrganizationHealth,
  resolvePrimaryAction,
} from "./HomeDashboard";
import {
  markOnboardingCompleted,
  resetOnboardingCompleted,
} from "./onboardingStorage";
import type {
  MonitoringDashboard,
  OrganizationOperation,
  RegisteredRoot,
  ScanResult,
  SystemStatus,
} from "./types";

vi.mock("./api", () => ({
  restoreWorkspaceSession: vi.fn().mockResolvedValue(null),
  getSystemStatus: vi.fn().mockResolvedValue({
    localFirst: true,
    readOnlyScan: true,
    networkDisabled: true,
    applyEnabled: false,
    recoveryRequired: false,
    journalLocked: false,
    journalDiagnostics: [],
    version: "0.1.0",
  }),
  createWorkspace: vi.fn(),
  selectAndRegisterRoot: vi.fn(),

  listUserContentLocations: vi.fn().mockResolvedValue([]),
  registerUserContentRoot: vi.fn(),
  scanWorkspace: vi.fn(),
  cancelScan: vi.fn(),
  analyzeContent: vi.fn(),
  analyzeSemantics: vi.fn(),
  cancelContentAnalysis: vi.fn(),
  cancelSemanticAnalysis: vi.fn(),
  subscribeScanProgress: vi.fn().mockResolvedValue(() => undefined),
  subscribeContentAnalysisProgress: vi.fn().mockResolvedValue(() => undefined),
  subscribeSemanticAnalysisProgress: vi.fn().mockResolvedValue(() => undefined),
  subscribeIdentityResolutionProgress: vi.fn().mockResolvedValue(() => undefined),
  listScanFiles: vi.fn().mockResolvedValue([]),
  listScanDuplicates: vi.fn().mockResolvedValue([]),
  listScanErrors: vi.fn().mockResolvedValue([]),
  listContentResults: vi.fn().mockResolvedValue([]),
  searchLocalFiles: vi.fn().mockResolvedValue({
    query: "",
    page: 0,
    pageSize: 50,
    total: 0,
    hasMore: false,
    results: [],
    interpretedQuery: [],
    embeddings: {
      availability: "unavailable",
      providerId: "none",
      version: "0",
      productionReady: false,
      indexedFiles: 0,
    },
    timings: {
      totalMs: 0,
      lexicalAndStructuredMs: 0,
      queryEmbedMs: 0,
      annMs: 0,
      vectorMs: 0,
      fusionMs: 0,
    },
  }),
  getEmbeddingModelStatus: vi.fn().mockResolvedValue({
    modelId: "granite-embedding-97m-multilingual-r2",
    version: "test",
    dimensions: 384,
    status: "not_installed",
    approximateDiskBytes: 1,
    license: "Apache-2.0",
    localOnly: true,
    downloadImplemented: true,
    lastError: null,
    installRoot: "/tmp/models",
  }),
  activateLocalEmbeddingModel: vi.fn(),
  cancelLocalEmbeddingModelInstall: vi.fn(),
  retryLocalEmbeddingModel: vi.fn(),
  removeLocalEmbeddingModel: vi.fn(),
  rebuildSemanticAnnIndex: vi.fn(),
  listReviewItems: vi.fn().mockResolvedValue({
    total: 0,
    limit: 50,
    offset: 0,
    hasMore: false,
    items: [],
  }),
  listIdentityReviewGroups: vi.fn().mockResolvedValue({
    total: 0,
    limit: 30,
    offset: 0,
    hasMore: false,
    items: [],
  }),
  getLatestOrganizationProposal: vi.fn().mockResolvedValue(null),
  generateOrganizationProposal: vi.fn(),
  cancelOrganizationProposal: vi.fn(),
  subscribeOrganizationProposalProgress: vi.fn().mockResolvedValue(() => undefined),
  getOrganizationProposal: vi.fn(),
  setOrganizationProposalOverride: vi.fn(),
  setOrganizationProposalStatus: vi.fn(),
  refreshOrganizationProposalDrift: vi.fn(),
  getRulesPreferences: vi.fn().mockResolvedValue({
    rules: [],
    suggestions: [],
    preferences: {
      clientFirst: true,
      includeYearFolders: true,
      maximumDepth: 6,
      minimumGroupSize: 2,
      keepPhotosInsideProjects: true,
      supplierInvoicesInsideProjects: true,
      namingLanguage: "fr",
      preserveExistingFolders: true,
      personalRootName: "Personnel",
      businessRootName: "Professionnel",
      renameTemplate: "{date}_{party}_{document_type}_{identifier}",
      reviewThreshold: 0.65,
    },
  }),
  acceptLocalRuleSuggestion: vi.fn(),
  createLocalRule: vi.fn(),
  deleteLocalRule: vi.fn(),
  dismissLocalRuleSuggestion: vi.fn(),
  recomputeRulesProposal: vi.fn(),
  reorderLocalRules: vi.fn(),
  setLocalRuleEnabled: vi.fn(),
  storeLocalOrganizationPreferences: vi.fn(),
  updateLocalRule: vi.fn(),
  getMonitoringDashboard: vi.fn(),
  pauseMonitoring: vi.fn(),
  resumeMonitoring: vi.fn(),
  runMonitoringCycle: vi.fn(),
  cancelMonitoring: vi.fn(),
  setMonitoredFolderEnabled: vi.fn(),
  addMonitoringExclusion: vi.fn(),
  removeMonitoringExclusion: vi.fn(),
  listExecutionHistory: vi.fn().mockResolvedValue([]),
  prepareExecution: vi.fn(),
  approveExecution: vi.fn(),
  startExecution: vi.fn(),
  pauseExecution: vi.fn(),
  cancelExecution: vi.fn(),
  getExecutionStatus: vi.fn(),
  rollbackExecution: vi.fn(),
  recoverExecution: vi.fn(),
  subscribeExecutionProgress: vi.fn().mockResolvedValue(() => undefined),
  retryExtraction: vi.fn(),
  cancelExtractionRetry: vi.fn(),
  redactPaths: (value: string) => value,
  getErrorMessage: (error: unknown) =>
    typeof error === "string" ? error : "Erreur locale",
  getErrorTechnicalDetails: () => null,
  getRawErrorText: (error: unknown) =>
    typeof error === "string" ? error : "raw",
}));

const system: SystemStatus = {
  localFirst: true,
  readOnlyScan: true,
  networkDisabled: true,
  applyEnabled: false,
  recoveryRequired: false,
  journalLocked: false,
  journalDiagnostics: [],
};

const root: RegisteredRoot = {
  id: "root-1",
  displayLabel: "Documents",
  selectedPath: "/Users/local/Documents",
};

const scan: ScanResult = {
  id: "scan-1",
  status: "COMPLETED",
  filesDiscovered: 12,
  filesIndexed: 12,
  directoriesDiscovered: 2,
  bytesDiscovered: 2048,
  filesHashed: 12,
  duplicateGroups: 1,
  errors: 0,
  skippedItems: 0,
  truncated: false,
};

function dashboard(
  overrides: Partial<MonitoringDashboard> = {},
): MonitoringDashboard {
  return {
    workspaceId: "workspace-1",
    mode: "PRUDENT",
    paused: false,
    startupReconciliationPending: false,
    automaticExecutionEnabled: false,
    folders: [
      {
        rootId: "root-1",
        displayLabel: "Documents",
        selectedPath: "/Users/local/Documents",
        enabled: true,
        status: "WATCHING",
        pendingJobs: 0,
      },
    ],
    counts: {
      filesAnalyzed: 12,
      readyToOrganize: 4,
      needsReview: 3,
      pendingProposals: 1,
      pendingJobs: 0,
    },
    recentActivity: [
      {
        id: "activity-1",
        summary: "3 nouveaux fichiers analysés",
        filesAnalyzed: 3,
        readyToOrganize: 1,
        needsReview: 1,
        failed: 0,
        createdAt: "2026-08-12T10:00:00Z",
      },
    ],
    exclusions: [],
    ...overrides,
  };
}

const noop = () => undefined;

function resetClientState() {
  cleanup();
  try {
    window.localStorage.clear();
  } catch {
    // jsdom 29 may expose a non-functional localStorage when
    // --localstorage-file is missing; onboarding uses a memory fallback.
  }
  try {
    window.sessionStorage.clear();
  } catch {
    // Same jsdom storage stub.
  }
  resetOnboardingCompleted();
}

describe("Milestone 12.1 dashboard command center", () => {
  afterEach(() => {
    resetClientState();
  });

  beforeEach(() => {
    resetClientState();
    vi.clearAllMocks();
    markOnboardingCompleted();
    vi.mocked(api.getMonitoringDashboard).mockResolvedValue(dashboard());
    vi.mocked(api.getLatestOrganizationProposal).mockResolvedValue(
      null as never,
    );
    vi.mocked(api.getEmbeddingModelStatus).mockResolvedValue({
      modelId: "granite-embedding-97m-multilingual-r2",
      version: "test",
      dimensions: 384,
      status: "not_installed",
      approximateDiskBytes: 1,
      license: "Apache-2.0",
      localOnly: true,
      downloadImplemented: true,
      lastError: null,
      installRoot: "/tmp/models",
    });
  });

  it("shows a new-user empty dashboard without zero-filled analytics", async () => {
    render(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId={null}
        root={null}
        scan={null}
        dashboard={null}
        dashboardError={false}
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={noop}
      />,
    );

    expect(screen.getByRole("heading", { name: "Bonjour" })).toBeTruthy();
    expect(
      screen.getByText(/Choisissez ce que vous voulez analyser/i),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Organiser mon ordinateur" }),
    ).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "État de l’organisation" })).toBeNull();
    expect(screen.queryByText("0")).toBeNull();
  });

  it("renders populated workspace status, attention, monitoring and local AI", async () => {
    render(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={dashboard()}
        dashboardError={false}
        contentNeedsReview={3}
        contentFailed={1}
        contentUnsupported={1}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={noop}
      />,
    );

    expect(screen.getByText(/fichiers analysés/i)).toBeTruthy();
    expect(screen.getAllByText(/à vérifier/i).length).toBeGreaterThan(0);
    expect(screen.getByRole("heading", { name: "À vérifier" })).toBeTruthy();
    expect(screen.getByRole("heading", { name: "Surveillance" })).toBeTruthy();
    expect(screen.getByText(/prépare des propositions uniquement/i)).toBeTruthy();
    await waitFor(() => {
      expect(api.getEmbeddingModelStatus).toHaveBeenCalled();
    });
    expect(await screen.findByText("Non activée")).toBeTruthy();
    expect(
      screen.getByText(/connexion peut être utilisée pour télécharger/i),
    ).toBeTruthy();
    expect(
      screen.queryByText(/rangés automatiquement|Classé dans/i),
    ).toBeNull();
  });

  it("adapts the primary CTA and surfaces monitoring issues", () => {
    expect(
      resolvePrimaryAction({
        root: null,
        scan: null,
        dashboard: null,
        contentNeedsReview: null,
      }).label,
    ).toBe("Organiser mon ordinateur");
    expect(
      resolvePrimaryAction({
        root,
        scan: null,
        dashboard: null,
        contentNeedsReview: null,
      }).label,
    ).toBe("Organiser mon ordinateur");
    expect(
      resolvePrimaryAction({
        root,
        scan,
        dashboard: dashboard({
          counts: {
            filesAnalyzed: 20,
            readyToOrganize: 0,
            needsReview: 23,
            pendingProposals: 0,
            pendingJobs: 0,
          },
        }),
        contentNeedsReview: null,
      }).label,
    ).toBe("Vérifier 23 éléments");
    expect(
      resolvePrimaryAction({
        root,
        scan,
        dashboard: dashboard({
          counts: {
            filesAnalyzed: 20,
            readyToOrganize: 0,
            needsReview: 0,
            pendingProposals: 0,
            pendingJobs: 0,
          },
        }),
        contentNeedsReview: 0,
      }).label,
    ).toBe("Voir l’organisation proposée");

    const issue = dashboard({
      folders: [
        {
          rootId: "root-1",
          displayLabel: "Documents",
          selectedPath: "/Users/local/Documents",
          enabled: true,
          status: "OFFLINE",
          pendingJobs: 0,
        },
      ],
    });
    render(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={issue}
        dashboardError={false}
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={noop}
      />,
    );
    expect(
      screen.getByText(/Dossier de surveillance indisponible/i),
    ).toBeTruthy();
  });

  it("shows unavailable metrics as em dash and documents health formula", () => {
    const unavailable = resolveOrganizationHealth({
      filesAnalyzed: null,
      needsReview: null,
      countsAvailable: false,
    });
    expect(unavailable.percentage).toBeNull();
    expect(unavailable.label).toMatch(/indisponible/i);

    const healthy = resolveOrganizationHealth({
      filesAnalyzed: 100,
      needsReview: 0,
      countsAvailable: true,
    });
    expect(healthy.percentage).toBe(100);
    expect(healthy.label).toBe("Très bien organisé");

    const watch = resolveOrganizationHealth({
      filesAnalyzed: 100,
      needsReview: 5,
      countsAvailable: true,
    });
    expect(watch.percentage).toBe(95);
    expect(watch.label).toBe("Quelques éléments à vérifier");

    render(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={null}
        dashboardError={false}
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={noop}
      />,
    );
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });

  it("navigates search with the typed query and keeps proposed destination wording", async () => {
    const onSearch = vi.fn();
    const onNavigate = vi.fn();
    const operation: OrganizationOperation = {
      id: "op-1",
      fileId: "file-1",
      fileVersionId: "fv-1",
      sourceRelativePath: "inbox/facture_pointp.pdf",
      sourceName: "facture_pointp.pdf",
      sourceByteSize: 12,
      machineDestination: ["Entreprise", "Fournisseurs", "Point P"],
      machineName: "facture_pointp.pdf",
      proposedDestination: [
        "Entreprise",
        "Fournisseurs",
        "Point P",
        "Factures",
        "2026",
      ],
      proposedName: "facture_pointp.pdf",
      proposedRelativePath:
        "Entreprise/Fournisseurs/Point P/Factures/2026/facture_pointp.pdf",
      operationKind: "MOVE_PROPOSAL",
      confidenceScore: 0.9,
      confidenceLevel: "HIGH",
      reasons: [],
      conflictState: "NONE",
      needsReview: false,
      stale: false,
      userOverride: false,
      disruptionScore: 0.1,
      proposedPathLength: 40,
      proposedDepth: 5,
      semanticContext: "business",
      documentType: "invoice",
      duplicateCanonical: true,
    };
    expect(formatProposedDestination(operation)).toContain("Point P");

    vi.mocked(api.getLatestOrganizationProposal).mockResolvedValue({
      id: "proposal-1",
      revisionId: "rev-1",
      workspaceId: "workspace-1",
      rootId: "root-1",
      sourceScanId: "scan-1",
      revision: 1,
      status: "READY_FOR_REVIEW",
      engineVersion: "1",
      policyVersion: "1",
      createdAt: "2026-08-12T10:00:00Z",
      updatedAt: "2026-08-12T10:00:00Z",
      summary: {
        filesAnalyzed: 12,
        proposedMoves: 1,
        proposedRenames: 0,
        unchanged: 0,
        needsReview: 0,
        unresolved: 0,
        conflicts: 0,
        highConfidence: 1,
        mediumConfidence: 0,
        lowConfidence: 0,
        duplicateNoAction: 0,
        averageDepth: 3,
        maximumDepth: 5,
      },
      change: {
        destinationsChanged: 1,
        filesAdded: 1,
        conflictsResolved: 0,
        movedToReview: 0,
      },
      nodes: [],
      operations: [operation],
    });

    render(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={dashboard()}
        dashboardError={false}
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={onNavigate}
        onSearch={onSearch}
        onRetryDashboard={noop}
      />,
    );

    fireEvent.change(
      screen.getByPlaceholderText(/Rechercher une facture, une photo, un devis/i),
      { target: { value: "facture Point P du chantier Martin" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Rechercher" }));
    expect(onSearch).toHaveBeenCalledWith(
      "facture Point P du chantier Martin",
    );

    expect(
      await screen.findByText(/Destination proposée/i),
    ).toBeTruthy();
    expect(screen.queryByText(/Classé dans/i)).toBeNull();
    expect(screen.getByText(/Emplacement actuel/i)).toBeTruthy();

    fireEvent.click(
      screen.getByRole("button", { name: "Ouvrir À revoir" }),
    );
    expect(onNavigate).toHaveBeenCalledWith("review");
  });

  it("bounds recent activity and shows error + loading states", () => {
    const many = Array.from({ length: 30 }, (_, index) => ({
      id: `activity-${index}`,
      summary: `événement ${index}`,
      filesAnalyzed: 1,
      readyToOrganize: 0,
      needsReview: 0,
      failed: 0,
      createdAt: "2026-08-12T10:00:00Z",
    }));
    const { rerender } = render(
      <HomeDashboard
        loading
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={dashboard({ recentActivity: many })}
        dashboardError={false}
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={noop}
      />,
    );
    expect(screen.getByText(/Chargement…/i)).toBeTruthy();

    const onRetry = vi.fn();
    rerender(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={null}
        dashboardError
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={onRetry}
      />,
    );
    expect(
      screen.getByText(/Impossible de charger l’état d’accueil/i),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Réessayer" }));
    expect(onRetry).toHaveBeenCalled();

    rerender(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={dashboard({ recentActivity: many })}
        dashboardError={false}
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={noop}
      />,
    );
    const activity = screen.getByRole("heading", {
      name: "Activité récente",
    }).closest("section");
    expect(activity).toBeTruthy();
    expect(within(activity as HTMLElement).getAllByRole("listitem").length).toBe(
      MAX_RECENT_ACTIVITY,
    );
    expect(MAX_RECENT_ACTIVITY).toBeLessThanOrEqual(10);
  });

  it("wires home search into the Search view with preserved query", async () => {
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue({
      workspace: { id: "workspace-1", name: "Local" },
      root,
      scan,
      safeReadOnly: true,
      filesystemExecutionResumed: false,
    });

    render(<App />);
    await screen.findByRole("heading", { name: "Bonjour" });

    fireEvent.change(
      screen.getByPlaceholderText(/Rechercher une facture, une photo, un devis/i),
      { target: { value: "devis Dupont de 2026" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Rechercher" }));

    await waitFor(() => {
      expect(api.searchLocalFiles).toHaveBeenCalled();
    });
    const searchInput = await screen.findByDisplayValue("devis Dupont de 2026");
    expect(searchInput).toBeTruthy();
  });

  it("keeps attention actions keyboard reachable", () => {
    render(
      <HomeDashboard
        loading={false}
        system={system}
        workspaceId="workspace-1"
        root={root}
        scan={scan}
        dashboard={dashboard()}
        dashboardError={false}
        contentNeedsReview={null}
        contentFailed={null}
        contentUnsupported={null}
        onPrimaryAction={noop}
        onNavigate={noop}
        onSearch={noop}
        onRetryDashboard={noop}
      />,
    );
    const review = screen.getByRole("button", { name: "Ouvrir À revoir" });
    review.focus();
    expect(document.activeElement).toBe(review);
    expect(
      screen.getByLabelText(/Rechercher une facture, une photo, un devis/i),
    ).toBeTruthy();
  });
});
