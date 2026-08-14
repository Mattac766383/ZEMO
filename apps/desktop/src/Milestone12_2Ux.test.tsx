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
import { OnboardingView } from "./OnboardingView";
import {
  markOnboardingCompleted,
  resetOnboardingCompleted,
} from "./onboardingStorage";
import type { UserContentLocation } from "./types";

const locations: UserContentLocation[] = [
  {
    kind: "desktop",
    displayLabel: "Bureau",
    absolutePath: "/Users/local/Desktop",
    exists: true,
    readable: true,
    recommended: true,
  },
  {
    kind: "documents",
    displayLabel: "Documents",
    absolutePath: "/Users/local/Documents",
    exists: true,
    readable: true,
    recommended: true,
  },
  {
    kind: "downloads",
    displayLabel: "Téléchargements",
    absolutePath: "/Users/local/Downloads",
    exists: true,
    readable: true,
    recommended: true,
  },
  {
    kind: "pictures",
    displayLabel: "Images",
    absolutePath: "/Users/local/Pictures",
    exists: true,
    readable: false,
    recommended: true,
  },
  {
    kind: "movies",
    displayLabel: "Vidéos",
    absolutePath: "/Users/local/Movies",
    exists: true,
    readable: true,
    recommended: false,
  },
  {
    kind: "music",
    displayLabel: "Musique",
    absolutePath: "/Users/local/Music",
    exists: false,
    readable: false,
    recommended: false,
  },
];

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
  createWorkspace: vi.fn().mockResolvedValue({
    id: "workspace-1",
    name: "Inventaire local",
  }),
  selectAndRegisterRoot: vi.fn(),
  listUserContentLocations: vi.fn(),
  registerUserContentRoot: vi.fn(),
  scanWorkspace: vi.fn().mockResolvedValue({
    id: "scan-1",
    status: "COMPLETED",
    filesDiscovered: 10,
    filesIndexed: 10,
    directoriesDiscovered: 2,
    bytesDiscovered: 1000,
    filesHashed: 10,
    duplicateGroups: 0,
    errors: 0,
    skippedItems: 0,
    truncated: false,
  }),
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
    modelId: "granite",
    version: "test",
    dimensions: 384,
    status: "not_installed",
    approximateDiskBytes: 118_000_000,
    license: "Apache-2.0",
    localOnly: true,
    downloadImplemented: true,
    lastError: null,
    installRoot: "/tmp",
  }),
  activateLocalEmbeddingModel: vi.fn().mockResolvedValue({
    modelId: "granite",
    version: "test",
    dimensions: 384,
    status: "ready",
    approximateDiskBytes: 118_000_000,
    license: "Apache-2.0",
    localOnly: true,
    downloadImplemented: true,
    lastError: null,
    installRoot: "/tmp",
  }),
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
      filesAnalyzed: 10,
      readyToOrganize: 4,
      needsReview: 0,
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
  retryExtraction: vi.fn(),
  cancelExtractionRetry: vi.fn(),
  redactPaths: (value: string) => value,
  getErrorMessage: (error: unknown) =>
    typeof error === "string" ? error : "Erreur locale",
  getErrorTechnicalDetails: () => null,
  getRawErrorText: (error: unknown) =>
    typeof error === "string" ? error : "raw",
}));

describe("Milestone 12.2 zero-friction UX + whole computer", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    resetOnboardingCompleted();
    vi.mocked(api.listUserContentLocations).mockResolvedValue(locations);
    vi.mocked(api.registerUserContentRoot).mockImplementation(
      async (_workspaceId, kind) => {
        const location = locations.find((item) => item.kind === kind)!;
        if (kind === "pictures") {
          return {
            root: null,
            kind,
            displayLabel: location.displayLabel,
            absolutePath: location.absolutePath,
            status: "denied",
            message: "accès refusé",
          };
        }
        return {
          root: {
            id: `root-${kind}`,
            displayLabel: location.displayLabel,
            selectedPath: location.absolutePath,
          },
          kind,
          displayLabel: location.displayLabel,
          absolutePath: location.absolutePath,
          status: "registered",
          message: null,
        };
      },
    );
  });

  it("shows Organiser mon ordinateur and Choisir des dossiers on first run", async () => {
    render(<App />);
    expect(
      await within(await screen.findByRole("dialog")).findByRole("button", { name: "Organiser mon ordinateur" }),
    ).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Choisir des dossiers" }),
    ).toBeTruthy();
    expect(screen.queryByText(/embedding|ANN|Granite/i)).toBeNull();
  });

  it("shows whole-computer scope preview with safe roots and permission explanation", async () => {
    const onStart = vi.fn();
    render(
      <OnboardingView
        onSelectFolder={vi.fn()}
        onComplete={vi.fn()}
        onStartWholeComputer={onStart}
      />,
    );

    fireEvent.click(
      screen.getByRole("button", { name: "Organiser mon ordinateur" }),
    );
    expect(
      await screen.findByText(/accéder uniquement aux emplacements/i),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(await screen.findByText(/Bureau/)).toBeTruthy();
    expect(screen.getByText(/Documents/)).toBeTruthy();
    expect(screen.getByText(/fichiers système/i)).toBeTruthy();
    expect(screen.getByText(/disque entier n.est jamais parcouru/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Personnaliser" }));
    expect(screen.getByLabelText(/Musique/i)).toBeTruthy();
    fireEvent.click(
      screen.getByRole("button", { name: "Commencer l’analyse" }),
    );
    await waitFor(() => {
      expect(onStart).toHaveBeenCalled();
    });
    const kinds = onStart.mock.calls[0][0] as string[];
    expect(kinds).toContain("documents");
    expect(kinds).not.toContain("/");
  });

  it("runs whole computer with partial permission denial and no Apply", async () => {
    render(<App />);
    fireEvent.click(
      await within(await screen.findByRole("dialog")).findByRole("button", { name: "Organiser mon ordinateur" }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "Continuer" }));
    fireEvent.click(
      await screen.findByRole("button", { name: "Commencer l’analyse" }),
    );

    await waitFor(() => {
      expect(api.registerUserContentRoot).toHaveBeenCalled();
      expect(api.scanWorkspace).toHaveBeenCalled();
    });
    expect(
      await screen.findByText(/Certains dossiers n’ont pas pu être analysés/i),
    ).toBeTruthy();
    expect(await screen.findByText(/Images — accès refusé/i)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Apply|Appliquer/i })).toBeNull();
    expect(api.prepareExecution).not.toHaveBeenCalled();
  });

  it("keeps primary journey CTAs wired after onboarding completion", async () => {
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
        filesDiscovered: 10,
        filesIndexed: 10,
        directoriesDiscovered: 1,
        bytesDiscovered: 1,
        filesHashed: 10,
        duplicateGroups: 0,
        errors: 0,
        skippedItems: 0,
        truncated: false,
      },
      safeReadOnly: true,
      filesystemExecutionResumed: false,
    });

    render(<App />);
    expect(await screen.findByRole("heading", { name: "Bonjour" })).toBeTruthy();

    const nav = screen.getByRole("navigation", {
      name: "Navigation principale",
    });
    expect(within(nav).queryByRole("button", { name: "Exécution" })).toBeNull();
    fireEvent.click(within(nav).getByRole("button", { name: "Recherche" }));
    expect(
      await screen.findByRole("heading", { name: "Retrouvez vos fichiers" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Activer" })).toBeTruthy();
    expect(
      screen.getByText(/même sans connaître leur nom exact/i),
    ).toBeTruthy();

    fireEvent.click(within(nav).getByRole("button", { name: "Surveillance" }));
    expect(
      await screen.findByText(/prépare de nouvelles propositions/i),
    ).toBeTruthy();
    expect(
      screen.getAllByText(/ne sont pas déplacés automatiquement/i)[0],
    ).toBeTruthy();
  });

  it("requests no permissions before the user chooses a scope", async () => {
    render(<App />);
    await within(await screen.findByRole("dialog")).findByRole("button", { name: "Organiser mon ordinateur" });
    expect(api.selectAndRegisterRoot).not.toHaveBeenCalled();
    expect(api.registerUserContentRoot).not.toHaveBeenCalled();
    expect(api.listUserContentLocations).not.toHaveBeenCalled();
  });
});
