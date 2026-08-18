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
import { resetOnboardingCompleted } from "./onboardingStorage";

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
  probeUserContentAccess: vi.fn().mockResolvedValue([]),
  authorizeUserContentFolder: vi.fn(),
  registerUserContentRoot: vi.fn(),
  scanWorkspace: vi.fn(),
  cancelScan: vi.fn(),
  analyzeContent: vi.fn(),
  analyzeSemantics: vi.fn().mockResolvedValue({
    id: "semantic-1",
    scanId: "scan-1",
    status: "COMPLETED",
    filesQueued: 2,
    filesCompleted: 2,
    highConfidence: 1,
    needsReview: 1,
    unknown: 0,
    partial: 0,
    failed: 0,
  }),
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
    query: "facture Point P",
    page: 0,
    pageSize: 50,
    total: 1,
    hasMore: false,
    results: [
      {
        fileId: "file-1",
        filename: "facture-point-p.pdf",
        relativePath: "Downloads/facture-point-p.pdf",
        byteSize: 1200,
        duplicate: false,
        matchSource: "content",
        relevance: 0.9,
        snippet: "Facture Point P",
        whyMatched: ["Correspond au fournisseur Point P"],
      },
    ],
    interpretedQuery: [],
    embeddings: {
      availability: "unavailable",
      providerLabel: "none",
      productionReady: false,
    },
    timings: {
      totalMs: 12,
      lexicalMs: 8,
      vectorMs: 0,
      fusionMs: 4,
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
    total: 1,
    limit: 50,
    offset: 0,
    hasMore: false,
    items: [
      {
        reviewId: "review-1",
        fileId: "file-2",
        filename: "inconnu.bin",
        relativePath: "Downloads/inconnu.bin",
        reason: "SEMANTIC_AMBIGUOUS",
        sourceSubsystem: "SEMANTIC",
        severity: "WARNING",
        explanation: "Emplacement incertain — à vérifier.",
        status: "NEEDS_REVIEW",
        retryAvailable: false,
        retryCount: 0,
        extractionStatus: "SUCCESS",
        createdAt: "2026-08-12T10:00:00Z",
        updatedAt: "2026-08-12T10:00:00Z",
      },
    ],
  }),
  listIdentityReviewGroups: vi.fn().mockResolvedValue({
    total: 0,
    limit: 30,
    offset: 0,
    hasMore: false,
    items: [],
  }),
  getLatestOrganizationProposal: vi.fn().mockResolvedValue({
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
      filesAnalyzed: 2,
      proposedMoves: 1,
      proposedRenames: 0,
      unchanged: 0,
      needsReview: 1,
      unresolved: 0,
      conflicts: 0,
      highConfidence: 1,
      mediumConfidence: 0,
      lowConfidence: 1,
      duplicateNoAction: 0,
      averageDepth: 2,
      maximumDepth: 3,
    },
    change: {
      destinationsChanged: 1,
      filesAdded: 0,
      conflictsResolved: 0,
      movedToReview: 1,
    },
    nodes: [],
    operations: [
      {
        id: "op-ready",
        fileId: "file-1",
        fileVersionId: "fv-1",
        sourceRelativePath: "Downloads/facture-point-p.pdf",
        sourceName: "facture-point-p.pdf",
        sourceByteSize: 1200,
        machineDestination: ["Professionnel", "Fournisseurs"],
        machineName: "facture-point-p.pdf",
        proposedDestination: ["Professionnel", "Fournisseurs"],
        proposedName: "facture-point-p.pdf",
        proposedRelativePath: "Professionnel/Fournisseurs/facture-point-p.pdf",
        operationKind: "MOVE_PROPOSAL",
        confidenceScore: 0.94,
        confidenceLevel: "VERY_HIGH",
        reasons: [
          {
            code: "supplier",
            explanation: "Fournisseur Point P détecté",
            evidenceReferences: [],
          },
        ],
        conflictState: "NONE",
        needsReview: false,
        stale: false,
      },
      {
        id: "op-review",
        fileId: "file-2",
        fileVersionId: "fv-2",
        sourceRelativePath: "Downloads/inconnu.bin",
        sourceName: "inconnu.bin",
        sourceByteSize: 40,
        machineDestination: ["TO_REVIEW"],
        machineName: "inconnu.bin",
        proposedDestination: ["TO_REVIEW"],
        proposedName: "inconnu.bin",
        proposedRelativePath: "TO_REVIEW/inconnu.bin",
        operationKind: "TO_REVIEW",
        confidenceScore: 0.4,
        confidenceLevel: "LOW",
        reasons: [],
        conflictState: "NONE",
        needsReview: true,
        stale: false,
      },
    ],
  }),
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
  getMonitoringDashboard: vi.fn().mockResolvedValue({
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
      filesAnalyzed: 2,
      readyToOrganize: 1,
      needsReview: 1,
      pendingProposals: 1,
      pendingJobs: 0,
    },
    recentActivity: [],
    exclusions: [],
  }),
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
  updateReviewItem: vi.fn(),
  getFileDetail: vi.fn(),
  storeSemanticCorrection: vi.fn(),
  retryExtraction: vi.fn(),
  cancelExtractionRetry: vi.fn(),
  redactPaths: (value: string) => value,
  getErrorMessage: () => "Erreur locale",
  getErrorTechnicalDetails: () => null,
  getRawErrorText: () => "raw",
}));

describe("Milestone 12 non-technical user walkthrough", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    resetOnboardingCompleted();
    Object.defineProperty(window, "innerWidth", {
      configurable: true,
      value: 1280,
    });
    Object.defineProperty(window, "innerHeight", {
      configurable: true,
      value: 800,
    });
    vi.mocked(api.createWorkspace).mockResolvedValue({
      id: "workspace-1",
      name: "Inventaire local",
    });
    vi.mocked(api.selectAndRegisterRoot).mockResolvedValue({
      id: "root-1",
      displayLabel: "Documents",
      selectedPath: "/Users/local/Documents",
    });
    vi.mocked(api.scanWorkspace).mockResolvedValue({
      id: "scan-1",
      status: "COMPLETED",
      filesDiscovered: 2,
      filesIndexed: 2,
      directoriesDiscovered: 1,
      bytesDiscovered: 2048,
      filesHashed: 2,
      duplicateGroups: 0,
      errors: 0,
      skippedItems: 0,
      truncated: false,
    });
  });

  it("walks a first-time user from onboarding to search and monitoring", async () => {
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: "ZEMO range vos fichiers, pas vos applications.",
      }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Choisir les dossiers" }));
    await waitFor(() => {
      expect(api.selectAndRegisterRoot).toHaveBeenCalled();
    });

    expect(
      await screen.findByRole("heading", { name: "Votre ordinateur est en bazar ?" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Ranger mon ordinateur" })).toBeTruthy();

    const nav = screen.getByRole("navigation", {
      name: "Navigation principale",
    });
    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(within(nav).getByRole("button", { name: "Inventaire" }));
    fireEvent.click(screen.getByRole("button", { name: "Scanner" }));
    expect(
      await screen.findByRole("heading", { name: "Analyse terminée" }),
    ).toBeTruthy();
    expect(
      screen.getByText(/fichiers analysés\. Rien n’a encore été modifié/i),
    ).toBeTruthy();

    const advanced = screen.getByText("Options avancées").closest("details");
    if (advanced) {
      advanced.open = true;
    }
    fireEvent.click(within(nav).getByRole("button", { name: "Organisation détaillée" }));
    expect(
      await screen.findByText(
        "Rien n’a encore été modifié sur votre ordinateur.",
      ),
    ).toBeTruthy();
    expect(screen.getByText(/À vérifier :/)).toBeTruthy();
    expect(screen.getAllByText("À vérifier").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Confiance élevée").length).toBeGreaterThan(0);

    if (advanced) {
      advanced.open = true;
    }
    fireEvent.click(within(nav).getByRole("button", { name: "À revoir" }));
    expect(
      await screen.findByText("Emplacement incertain — à vérifier."),
    ).toBeTruthy();

    fireEvent.click(within(nav).getByRole("button", { name: "Recherche" }));
    const search = await screen.findByLabelText(/^Recherche$/i);
    fireEvent.change(search, {
      target: { value: "facture Point P" },
    });
    await waitFor(() => {
      expect(api.searchLocalFiles).toHaveBeenCalled();
    });
    expect(await screen.findByText("facture-point-p.pdf")).toBeTruthy();

    fireEvent.click(within(nav).getByRole("button", { name: "Surveillance" }));
    expect(
      await screen.findByRole("heading", { name: "Surveillance" }),
    ).toBeTruthy();
    expect(
      screen.getAllByText(/ne sont pas déplacés automatiquement/i)[0],
    ).toBeTruthy();
    expect(screen.getByText("Saine")).toBeTruthy();
    expect(
      screen.queryByRole("button", {
        name: /^(move|rename|delete|apply|exécuter)$/i,
      }),
    ).toBeNull();
  });

  it("keeps the essential keyboard flow reachable", async () => {
    render(<App />);
    const primary = await within(await screen.findByRole("dialog")).findByRole(
      "button",
      { name: "Continuer" },
    );
    primary.focus();
    expect(document.activeElement).toBe(primary);
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Choisir les dossiers" }));

    const cta = await screen.findByRole("button", {
      name: "Ranger mon ordinateur",
    });
    expect((cta as HTMLButtonElement).disabled).toBe(false);
    expect(cta.getAttribute("type")).toBe("button");
    cta.focus();
    expect(document.activeElement === cta || cta.matches(":focus")).toBeTruthy();

    const nav = screen.getByRole("navigation", {
      name: "Navigation principale",
    });
    fireEvent.click(screen.getByText("Options avancées"));
    const filesNav = within(nav).getByRole("button", { name: "Inventaire" });
    filesNav.focus();
    fireEvent.click(filesNav);
    expect(
      await screen.findByRole("heading", { name: "Dossier à analyser" }),
    ).toBeTruthy();
    const choose = screen.getByRole("button", { name: "Choisir un dossier" });
    expect((choose as HTMLButtonElement).disabled).toBe(false);
    expect(window.innerWidth).toBe(1280);
    expect(window.innerHeight).toBe(800);
  });
});
