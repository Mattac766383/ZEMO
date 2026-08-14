// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import App from "./App";
import { markOnboardingCompleted } from "./onboardingStorage";

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
  analyzeSemantics: vi.fn().mockResolvedValue({
    id: "semantic-batch",
    scanId: "scan",
    status: "COMPLETED",
    filesQueued: 0,
    filesCompleted: 0,
    highConfidence: 0,
    needsReview: 0,
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
    query: "",
    page: 0,
    pageSize: 50,
    total: 0,
    hasMore: false,
    results: [],
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
  updateReviewItem: vi.fn(),
  getFileDetail: vi.fn(),
  storeSemanticCorrection: vi.fn(),
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
      namingLanguage: "en",
      preserveExistingFolders: true,
      personalRootName: "Personal",
      businessRootName: "Business",
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
  getLatestOrganizationProposal: vi.fn().mockResolvedValue(null),
  pauseMonitoring: vi.fn(),
  resumeMonitoring: vi.fn(),
  runMonitoringCycle: vi.fn(),
  cancelMonitoring: vi.fn(),
  setMonitoredFolderEnabled: vi.fn(),
  addMonitoringExclusion: vi.fn(),
  removeMonitoringExclusion: vi.fn(),
  retryExtraction: vi.fn(),
  cancelExtractionRetry: vi.fn(),
  redactPaths: (value: string) => value,
  getErrorMessage: () => "Erreur locale",
  getErrorTechnicalDetails: () => null,
}));

describe("safe scanner desktop workflow", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    markOnboardingCompleted();
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue(null);
    vi.mocked(api.getSystemStatus).mockResolvedValue({
      localFirst: true,
      readOnlyScan: true,
      networkDisabled: true,
      applyEnabled: false,
      recoveryRequired: false,
      journalLocked: false,
      journalDiagnostics: [],
      version: "0.1.0",
    });
    vi.mocked(api.subscribeScanProgress).mockResolvedValue(() => undefined);
    vi.mocked(api.subscribeContentAnalysisProgress).mockResolvedValue(
      () => undefined,
    );
    vi.mocked(api.subscribeSemanticAnalysisProgress).mockResolvedValue(
      () => undefined,
    );
    vi.mocked(api.subscribeIdentityResolutionProgress).mockResolvedValue(
      () => undefined,
    );
    vi.mocked(api.listIdentityReviewGroups).mockResolvedValue({
      total: 0,
      limit: 30,
      offset: 0,
      hasMore: false,
      items: [],
    });
    vi.mocked(api.getMonitoringDashboard).mockResolvedValue({
      workspaceId: "workspace-restored",
      mode: "PRUDENT",
      paused: false,
      startupReconciliationPending: false,
      automaticExecutionEnabled: false,
      folders: [
        {
          rootId: "root-restored",
          displayLabel: "Documents",
          selectedPath: "/Users/local/Documents",
          enabled: true,
          status: "WATCHING",
          pendingJobs: 0,
          lastReconciledAt: "2026-08-11T12:00:00Z",
        },
      ],
      counts: {
        filesAnalyzed: 1,
        readyToOrganize: 1,
        needsReview: 1,
        pendingProposals: 1,
        pendingJobs: 0,
      },
      recentActivity: [],
      exclusions: [],
    });
  });

  it("shows explicit scope and read-only privacy guarantees", async () => {
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: "Organisez et retrouvez vos fichiers.",
      }),
    ).toBeTruthy();
    expect(await screen.findByText("Traitement local")).toBeTruthy();
    expect(screen.getByText("Rien n’est déplacé au scan")).toBeTruthy();
    expect(screen.getByText("Aucun upload")).toBeTruthy();
    expect(
      screen.getByText(/ZEMO · Bêta privée macOS · 0.1.0-beta.5/i),
    ).toBeTruthy();
    expect(
      await screen.findByRole("heading", { name: "Bonjour" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Organiser mon ordinateur" }),
    ).toBeTruthy();
    expect(
      screen.getAllByText(/analysés localement|organisation proposée/i).length,
    ).toBeGreaterThan(0);
    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getByRole("button", { name: "Inventaire" }));
    expect(
      (screen.getByRole("button", { name: "Scanner" }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(screen.queryByRole("button", { name: /delete|move|rename/i })).toBeNull();
  });

  it("restores the current workspace and scan without resuming execution", async () => {
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue({
      workspace: {
        id: "workspace-restored",
        name: "Restored locally",
      },
      root: {
        id: "root-restored",
        displayLabel: "Documents",
        selectedPath: "/Users/local/Documents",
      },
      scan: {
        id: "scan-restored",
        status: "COMPLETED",
        filesDiscovered: 4,
        filesIndexed: 4,
        directoriesDiscovered: 1,
        bytesDiscovered: 4096,
        filesHashed: 4,
        duplicateGroups: 0,
        errors: 0,
        skippedItems: 0,
        truncated: false,
      },
      safeReadOnly: true,
      filesystemExecutionResumed: false,
    });

    render(<App />);

    expect(await screen.findByText("/Users/local/Documents")).toBeTruthy();
    expect(
      await screen.findByRole("heading", { name: "Bonjour" }),
    ).toBeTruthy();
    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getByRole("button", { name: "Inventaire" }));
    expect(
      await screen.findByRole("heading", { name: "Analyse terminée" }),
    ).toBeTruthy();
    expect(api.createWorkspace).not.toHaveBeenCalled();
  });

  it("opens persisted monitoring review after restart without a manual scan", async () => {
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue({
      workspace: {
        id: "workspace-restored",
        name: "Restored locally",
      },
      root: {
        id: "root-restored",
        displayLabel: "Documents",
        selectedPath: "/Users/local/Documents",
      },
      scan: null,
      safeReadOnly: true,
      filesystemExecutionResumed: false,
    });

    render(<App />);
    const nav = await screen.findByRole("navigation", {
      name: "Navigation principale",
    });
    fireEvent.click(
      Array.from(nav.querySelectorAll("button")).find(
        (button) => button.textContent === "Surveillance",
      )!,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Ouvrir À revoir" }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "Les fichiers qui demandent votre attention",
      }),
    ).toBeTruthy();
    expect(api.listReviewItems).toHaveBeenCalledWith(
      "workspace-restored",
      "needs_review",
      "all",
      50,
      0,
    );
    expect(api.scanWorkspace).not.toHaveBeenCalled();
  });

  it("selects one folder, scans it, and shows informational result views", async () => {
    vi.mocked(api.createWorkspace).mockResolvedValue({
      id: "workspace-1",
      name: "Inventaire local",
    });
    vi.mocked(api.selectAndRegisterRoot).mockResolvedValue({
      id: "root-1",
      displayLabel: "TestData",
      selectedPath: "C:\\Users\\User\\Documents\\TestData",
    });
    vi.mocked(api.scanWorkspace).mockResolvedValue({
      id: "scan-1",
      status: "COMPLETED",
      filesDiscovered: 3,
      filesIndexed: 3,
      directoriesDiscovered: 2,
      bytesDiscovered: 2048,
      filesHashed: 2,
      duplicateGroups: 1,
      errors: 0,
      skippedItems: 0,
      truncated: false,
    });
    vi.mocked(api.analyzeContent).mockResolvedValue({
      id: "analysis-1",
      scanId: "scan-1",
      status: "COMPLETED",
      filesQueued: 3,
      filesCompleted: 3,
      successful: 1,
      partial: 0,
      unsupported: 2,
      skipped: 0,
      failed: 0,
      ocrProcessed: 0,
    });
    vi.mocked(api.listContentResults).mockResolvedValue([
      {
        fileVersionId: "file-version-1",
        filename: "invoice.txt",
        relativePath: "invoice.txt",
        extension: "txt",
        status: "SUCCESS",
        extractorType: "plain_text",
        detectedContentType: "text/plain",
        typeMismatch: false,
        textPreview: "Invoice 2026",
        characterCount: 12,
        requiresOcr: false,
        ocrUsed: false,
        extractionDurationMs: 1,
        truncated: false,
        structuredMetadata: { network: false },
      },
    ]);
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
      query: "",
      page: 0,
      pageSize: 50,
      total: 1,
      hasMore: false,
      interpretedQuery: [],
      embeddings: {
        availability: "unavailable",
        providerId: "unavailable-local-embedding",
        version: "none",
        productionReady: false,
        indexedFiles: 0,
      },
      timings: {
        totalMs: 1,
        lexicalAndStructuredMs: 1,
        queryEmbedMs: 0,
        annMs: 0,
        vectorMs: 0,
        fusionMs: 0,
      },
      results: [
        {
          fileId: "file-1",
          filename: "invoice.txt",
          relativePath: "Clients/invoice.txt",
          extension: "txt",
          detectedType: "text/plain",
          byteSize: 2048,
          extractionStatus: "success",
          ocrStatus: "not_used",
          duplicate: false,
          matchSource: "content",
          relevance: 1,
          snippet: "Invoice 2026",
          whyMatched: ["texte du document correspondant"],
        },
      ],
    });
    vi.mocked(api.listReviewItems).mockResolvedValue({
      total: 1,
      limit: 50,
      offset: 0,
      hasMore: false,
      items: [
        {
          reviewId: "review-1",
          fileId: "file-1",
          filename: "scan.pdf",
          relativePath: "Scans/scan.pdf",
          reason: "OCR_PROVIDER_UNAVAILABLE",
          sourceSubsystem: "EXTRACTION",
          severity: "WARNING",
          explanation: "La reconnaissance locale du texte est indisponible.",
          status: "NEEDS_REVIEW",
          retryAvailable: true,
          retryCount: 0,
          extractionStatus: "PARTIAL",
          createdAt: "2026-08-10T10:00:00Z",
          updatedAt: "2026-08-10T10:00:00Z",
        },
      ],
    });

    render(<App />);
    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getByRole("button", { name: "Inventaire" }));
    fireEvent.click(screen.getByRole("button", { name: "Choisir un dossier" }));
    expect(
      await screen.findByText("C:\\Users\\User\\Documents\\TestData"),
    ).toBeTruthy();

    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getByRole("button", { name: "Inventaire" }));
    fireEvent.click(screen.getByRole("button", { name: "Scanner" }));
    expect(await screen.findByRole("heading", { name: "Analyse terminée" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Voir les fichiers" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Voir les doublons" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Voir les erreurs" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Analyser les documents" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Voir l’organisation" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Rechercher un fichier" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Rechercher un fichier" }));
    expect(await screen.findByRole("heading", { name: "Retrouvez vos fichiers" })).toBeTruthy();
    expect(await screen.findByText("Invoice 2026")).toBeTruthy();

    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getAllByRole("button", { name: "À revoir" })[0]);
    expect(
      await screen.findByText("La reconnaissance locale du texte est indisponible."),
    ).toBeTruthy();
    expect(
      await screen.findByRole("heading", {
        name: "Identités et projets à vérifier",
      }),
    ).toBeTruthy();

    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getByRole("button", { name: "Inventaire" }));
    fireEvent.click(screen.getByRole("button", { name: "Analyser les documents" }));
    expect(
      await screen.findByRole("heading", { name: "invoice.txt" }),
    ).toBeTruthy();
    expect(screen.getByText("Invoice 2026")).toBeTruthy();

    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getByRole("button", { name: "Préférences de rangement" }));
    expect(
      await screen.findByRole("heading", { name: "Préférences de rangement" }),
    ).toBeTruthy();
  });

  it("renders batched backend progress events", async () => {
    vi.mocked(api.subscribeScanProgress).mockImplementation(async (handler) => {
      handler({
        scanId: "scan-progress",
        phase: "HASHING",
        filesDiscovered: 12_482,
        filesIndexed: 9_731,
        directoriesDiscovered: 420,
        bytesDiscovered: 184 * 1024 ** 3,
        filesHashed: 9_201,
        duplicateGroups: 12,
        errors: 23,
        skippedItems: 7,
      });
      return () => undefined;
    });

    render(<App />);

    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(await screen.findByRole("button", { name: "Inventaire" }));
    expect(await screen.findByText("Préparation de l’organisation…")).toBeTruthy();
    expect(screen.getByText("12482")).toBeTruthy();
    expect(screen.getByText("9731")).toBeTruthy();
    expect(screen.getByText("9201")).toBeTruthy();
    expect(screen.getByText("23")).toBeTruthy();
  });
});
