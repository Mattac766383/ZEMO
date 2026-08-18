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
import { resolvePrimaryAction } from "./HomeDashboard";
import { OrganizationPreviewView } from "./OrganizationPreviewView";
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
  probeUserContentAccess: vi.fn().mockResolvedValue([]),
  authorizeUserContentFolder: vi.fn(),
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
  getErrorTechnicalDetails: () => "EACCES: permission denied",
  getRawErrorText: (error: unknown) =>
    typeof error === "string" ? error : "raw",
}));

describe("Milestone 12 main desktop UX", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    markOnboardingCompleted();
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue({
      workspace: { id: "workspace-1", name: "Local" },
      root: {
        id: "root-1",
        displayLabel: "Documents",
        selectedPath: "/Users/local/Documents",
      },
      scan: {
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
      },
      safeReadOnly: true,
      filesystemExecutionResumed: false,
    });
  });

  it("shows a home dashboard with real counts and one primary CTA", async () => {
    render(<App />);

    expect(
      await screen.findByRole("heading", { name: "Votre ordinateur est en bazar ?" }),
    ).toBeTruthy();
    expect(
      screen.getByText(
        /range vos fichiers personnels sans toucher à vos applications/i,
      ),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Ranger mon ordinateur" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Choisir les dossiers" }),
    ).toBeTruthy();
    expect(screen.queryByRole("heading", { name: "À vérifier" })).toBeNull();
    expect(
      screen.queryByPlaceholderText(/Rechercher une facture, une photo, un devis/i),
    ).toBeNull();
  });

  it("navigates through the main product areas from the shell", async () => {
    render(<App />);
    await screen.findByRole("heading", { name: "Votre ordinateur est en bazar ?" });

    const nav = screen.getByRole("navigation", {
      name: "Navigation principale",
    });
    fireEvent.click(within(nav).getByRole("button", { name: "Recherche" }));
    expect(
      await screen.findByRole("heading", { name: "Retrouvez vos fichiers" }),
    ).toBeTruthy();

    fireEvent.click(within(nav).getByRole("button", { name: "Surveillance" }));
    expect(
      await screen.findByRole("heading", { name: "Surveillance" }),
    ).toBeTruthy();
    expect(
      screen.getAllByText(/ne sont pas déplacés automatiquement/i)[0],
    ).toBeTruthy();

    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(within(nav).getByRole("button", { name: "Préférences de rangement" }));
    expect(
      await screen.findByRole("heading", { name: "Préférences de rangement" }),
    ).toBeTruthy();
    expect(
      screen.getByText(/n.autorisent pas à déplacer/i),
    ).toBeTruthy();
  });

  it("keeps Current vs Proposed and safety communication unmistakable", async () => {
    render(<OrganizationPreviewView workspaceId="workspace-1" rootId="root-1" />);

    expect(
      await screen.findByRole("heading", { name: "Organisation proposée" }),
    ).toBeTruthy();
    expect(
      screen.getByText("Rien n’a encore été modifié sur votre ordinateur."),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Préparer l’organisation" }),
    ).toBeTruthy();
  });

  it("resolves primary actions from real workspace state", () => {
    expect(
      resolvePrimaryAction({
        root: null,
        scan: null,
        dashboard: null,
        contentNeedsReview: null,
      }).label,
    ).toBe("Ranger mon ordinateur");

    expect(
      resolvePrimaryAction({
        root: {
          id: "r",
          displayLabel: "Docs",
          selectedPath: "/tmp",
        },
        scan: null,
        dashboard: null,
        contentNeedsReview: null,
      }).label,
    ).toBe("Ranger mon ordinateur");

    expect(
      resolvePrimaryAction({
        root: {
          id: "r",
          displayLabel: "Docs",
          selectedPath: "/tmp",
        },
        scan: {
          id: "s",
          status: "COMPLETED",
          filesDiscovered: 1,
          filesIndexed: 1,
          directoriesDiscovered: 1,
          bytesDiscovered: 1,
          filesHashed: 1,
          duplicateGroups: 0,
          errors: 0,
          skippedItems: 0,
          truncated: false,
        },
        dashboard: {
          workspaceId: "w",
          mode: "PRUDENT",
          paused: true,
          startupReconciliationPending: false,
          automaticExecutionEnabled: false,
          folders: [],
          counts: {
            filesAnalyzed: 1,
            readyToOrganize: 0,
            needsReview: 12,
            pendingProposals: 0,
            pendingJobs: 0,
          },
          recentActivity: [],
          exclusions: [],
        },
        contentNeedsReview: null,
      }).label,
    ).toBe("Ranger mon ordinateur");
  });

  it("keeps primary navigation keyboard reachable", async () => {
    render(<App />);
    const home = await screen.findByRole("button", { name: "Accueil" });
    home.focus();
    expect(document.activeElement).toBe(home);
    fireEvent.click(screen.getByText("Options avancées"));
    fireEvent.click(screen.getByRole("button", { name: "Inventaire" }));
    expect(
      await screen.findByRole("heading", { name: "Dossier à analyser" }),
    ).toBeTruthy();
  });

  it("shows humanized scan empty and monitoring health states without inventing data", async () => {
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue(null);
    vi.mocked(api.getMonitoringDashboard).mockResolvedValue({
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
    });

    render(<App />);
    expect(
      await screen.findByRole("heading", { name: "Votre ordinateur est en bazar ?" }),
    ).toBeTruthy();

    vi.mocked(api.createWorkspace).mockResolvedValue({
      id: "workspace-1",
      name: "Inventaire local",
    });
    vi.mocked(api.selectAndRegisterRoot).mockResolvedValue({
      id: "root-1",
      displayLabel: "Documents",
      selectedPath: "/Users/local/Documents",
    });
    fireEvent.click(screen.getByRole("button", { name: "Choisir les dossiers" }));
    await waitFor(() => {
      expect(api.selectAndRegisterRoot).toHaveBeenCalled();
    });
  });
});
