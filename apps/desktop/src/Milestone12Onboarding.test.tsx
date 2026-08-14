// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor , within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import App from "./App";
import {
  isOnboardingCompleted,
  markOnboardingCompleted,
  resetOnboardingCompleted,
} from "./onboardingStorage";

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
  listUserContentLocations: vi.fn().mockResolvedValue([
    {
      kind: "documents",
      displayLabel: "Documents",
      absolutePath: "/Users/local/Documents",
      exists: true,
      readable: true,
      recommended: true,
    },
  ]),
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
    modelId: "granite",
    version: "test",
    dimensions: 384,
    status: "not_installed",
    approximateDiskBytes: 1,
    license: "Apache-2.0",
    localOnly: true,
    downloadImplemented: true,
    lastError: null,
    installRoot: "/tmp",
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
  getLatestOrganizationProposal: vi.fn().mockRejectedValue(new Error("none")),
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
    folders: [],
    counts: {
      filesAnalyzed: 0,
      readyToOrganize: 0,
      needsReview: 0,
      pendingProposals: 0,
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
  retryExtraction: vi.fn(),
  cancelExtractionRetry: vi.fn(),
  redactPaths: (value: string) => value,
  getErrorMessage: (error: unknown) =>
    typeof error === "string" ? error : "Erreur locale",
  getErrorTechnicalDetails: () => null,
  getRawErrorText: (error: unknown) =>
    typeof error === "string" ? error : "raw",
}));

describe("Milestone 12 first-run onboarding", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    resetOnboardingCompleted();
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue(null);
    vi.mocked(api.createWorkspace).mockResolvedValue({
      id: "workspace-1",
      name: "Inventaire local",
      createdAt: "2026-08-12T00:00:00Z",
    });
    vi.mocked(api.selectAndRegisterRoot).mockResolvedValue({
      id: "root-1",
      selectedPath: "/Users/local/Documents",
      displayLabel: "Documents",
    });
  });

  it("shows the welcome step on first run", async () => {
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: "Organisez et retrouvez vos fichiers automatiquement.",
      }),
    ).toBeTruthy();
    expect(screen.getByRole("dialog")).toBeTruthy();
    expect(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Organiser mon ordinateur",
      }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Choisir des dossiers" }),
    ).toBeTruthy();
  });

  it("explains local analysis and proposal-only organization", async () => {
    render(<App />);
    await screen.findByRole("heading", {
      name: "Organisez et retrouvez vos fichiers automatiquement.",
    });
    expect(screen.getByText(/analysés localement/i)).toBeTruthy();
    expect(screen.getByText(/rien n’est déplacé automatiquement/i)).toBeTruthy();
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Organiser mon ordinateur",
      }),
    );
    expect(
      await screen.findByText(/accéder uniquement aux emplacements/i),
    ).toBeTruthy();
  });

  it("reuses folder selection from Choisir des dossiers", async () => {
    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Choisir des dossiers" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(
      screen.getByRole("button", { name: "Sélectionner un dossier" }),
    );

    await waitFor(() => {
      expect(api.createWorkspace).toHaveBeenCalled();
      expect(api.selectAndRegisterRoot).toHaveBeenCalledWith("workspace-1");
    });
    expect(
      screen.getAllByText("/Users/local/Documents").length,
    ).toBeGreaterThan(0);
  });

  it("persists completion and skips onboarding after restart", async () => {
    const { unmount } = render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Choisir des dossiers" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    fireEvent.click(screen.getByRole("button", { name: "Passer pour l’instant" }));

    await waitFor(() => {
      expect(isOnboardingCompleted()).toBe(true);
      expect(screen.queryByRole("dialog")).toBeNull();
    });

    unmount();
    render(<App />);

    expect(
      await screen.findByRole("heading", {
        name: "Organisez et retrouvez vos fichiers.",
      }),
    ).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("reopens the tour from Help without clearing completion", async () => {
    markOnboardingCompleted();
    render(<App />);
    expect(
      await screen.findByRole("heading", {
        name: "Organisez et retrouvez vos fichiers.",
      }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ouvrir la visite guidée" }));
    expect(
      await within(await screen.findByRole("dialog")).findByRole("button", { name: "Organiser mon ordinateur" }),
    ).toBeTruthy();
    expect(isOnboardingCompleted()).toBe(true);
  });

  it("supports keyboard focus on the primary CTA", async () => {
    render(<App />);
    const primary = await within(await screen.findByRole("dialog")).findByRole("button", {
      name: "Organiser mon ordinateur",
    });
    expect(document.activeElement).toBe(primary);
  });
});
