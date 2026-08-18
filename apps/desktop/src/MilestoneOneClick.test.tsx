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
import { clearLastOrganizeResult } from "./organizeStorage";
import { summarizeProposals } from "./oneClickSummary";
import type {
  FolderAccessProbe,
  OrganizationProposal,
  UserContentLocation,
} from "./types";

const locations: UserContentLocation[] = [
  {
    kind: "desktop",
    displayLabel: "Bureau",
    absolutePath: "/Users/local/Desktop",
    exists: true,
    readable: true,
    recommended: true,
  },
];

const proposal: OrganizationProposal = {
  id: "proposal-1",
  revisionId: "rev-1",
  workspaceId: "workspace-1",
  rootId: "root-desktop",
  sourceScanId: "scan-1",
  revision: 1,
  status: "READY_FOR_REVIEW",
  engineVersion: "1",
  policyVersion: "1",
  createdAt: "2026-08-18T10:00:00Z",
  updatedAt: "2026-08-18T10:00:00Z",
  summary: {
    filesAnalyzed: 8,
    proposedMoves: 7,
    proposedRenames: 0,
    unchanged: 1,
    needsReview: 1,
    unresolved: 0,
    conflicts: 0,
    highConfidence: 6,
    mediumConfidence: 0,
    lowConfidence: 1,
    duplicateNoAction: 0,
    averageDepth: 2,
    maximumDepth: 2,
  },
  change: {
    destinationsChanged: 7,
    filesAdded: 8,
    conflictsResolved: 0,
    movedToReview: 1,
  },
  nodes: [],
  operations: [
    op("invoice.pdf", ["Documents", "Administratif"]),
    op("holiday.jpg", ["Images", "Photos"]),
    op("video.mp4", ["Vidéos"]),
    op("archive.zip", ["Archives"]),
    op("setup.exe", ["Installateurs"]),
    op("unknown.xyz", ["À vérifier"]),
    op("notes.txt", ["Documents", "Personnel"]),
    {
      ...op("App.lnk", []),
      operationKind: "KEEP_IN_PLACE",
      proposedDestination: [],
    },
  ],
};

function toProbe(
  location: UserContentLocation,
  accessState = location.readable ? "accessible" : "authorization_required",
): FolderAccessProbe {
  return {
    logicalName: String(location.kind),
    kind: String(location.kind),
    displayLabel: location.displayLabel,
    resolvedPath: location.absolutePath,
    exists: location.exists,
    isDir: location.exists,
    readable: location.readable,
    writable: location.readable,
    recommended: location.recommended,
    accessState,
    humanStatus:
      accessState === "accessible"
        ? `✓ ${location.displayLabel}`
        : accessState === "authorization_required"
          ? `${location.displayLabel} — Autorisation nécessaire`
          : `${location.displayLabel} — Indisponible`,
  };
}

function op(name: string, destination: string[]) {
  return {
    id: `op-${name}`,
    fileId: `file-${name}`,
    fileVersionId: `fv-${name}`,
    sourceRelativePath: name,
    sourceName: name,
    sourceByteSize: 12,
    machineDestination: destination,
    machineName: name,
    proposedDestination: destination,
    proposedName: name,
    proposedRelativePath: [...destination, name].join("/"),
    operationKind: "MOVE_PROPOSAL" as const,
    confidenceScore: 0.9,
    confidenceLevel: "HIGH" as const,
    reasons: [],
    conflictState: "NONE",
    needsReview: destination[0] === "À vérifier",
    stale: false,
    userOverride: false,
    disruptionScore: 0.2,
    proposedPathLength: 20,
    proposedDepth: destination.length,
    semanticContext: "unknown",
    documentType: "unknown",
    duplicateCanonical: true,
  };
}

vi.mock("./api", () => ({
  restoreWorkspaceSession: vi.fn().mockResolvedValue(null),
  getSystemStatus: vi.fn().mockResolvedValue({
    localFirst: true,
    readOnlyScan: true,
    networkDisabled: true,
    applyEnabled: true,
    recoveryRequired: false,
    journalLocked: false,
    journalDiagnostics: [],
    version: "0.1.0",
  }),
  createWorkspace: vi.fn().mockResolvedValue({
    id: "workspace-1",
    name: "ZEMO",
  }),
  selectAndRegisterRoot: vi.fn(),
  listUserContentLocations: vi.fn(),
  probeUserContentAccess: vi.fn(),
  authorizeUserContentFolder: vi.fn(),
  registerUserContentRoot: vi.fn(),
  scanWorkspace: vi.fn().mockResolvedValue({
    id: "scan-1",
    status: "COMPLETED",
    filesDiscovered: 8,
    filesIndexed: 8,
    directoriesDiscovered: 1,
    bytesDiscovered: 1000,
    filesHashed: 8,
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
      includeYearFolders: false,
      maximumDepth: 3,
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

describe("ZEMO one-click organize", () => {
  afterEach(() => {
    cleanup();
    clearLastOrganizeResult();
  });

  beforeEach(() => {
    vi.clearAllMocks();
    resetOnboardingCompleted();
    clearLastOrganizeResult();
    vi.mocked(api.listUserContentLocations).mockResolvedValue(locations);
    vi.mocked(api.probeUserContentAccess).mockResolvedValue(
      locations.filter((item) => item.recommended).map((item) => toProbe(item)),
    );
    vi.mocked(api.registerUserContentRoot).mockImplementation(async (_id, kind) => {
      const location = locations.find((item) => item.kind === kind)!;
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
    });
    vi.mocked(api.generateOrganizationProposal).mockResolvedValue(proposal);
    vi.mocked(api.setOrganizationProposalStatus).mockImplementation(async (id) => ({
      ...proposal,
      id,
      status: "APPROVED_FOR_FUTURE_APPLY",
    }));
    vi.mocked(api.prepareExecution).mockResolvedValue({
      session: {
        id: "exec-1",
        status: "AWAITING_CONFIRMATION",
        summary: {
          preflightOk: 7,
          applied: 0,
          blocked: 0,
          skipped: 0,
          failed: 0,
          affectedFiles: 7,
        },
        approval: { userConfirmed: false, operationCount: 7 },
        rollbackAvailable: false,
        confirmationPhraseRequired: false,
        createdAt: "2026-08-18T10:00:00Z",
      },
      operations: [],
    } as never);
    vi.mocked(api.approveExecution).mockResolvedValue({
      session: {
        id: "exec-1",
        status: "APPROVED",
        summary: { applied: 0, preflightOk: 7 },
        approval: { userConfirmed: true },
        rollbackAvailable: false,
        confirmationPhraseRequired: false,
        createdAt: "2026-08-18T10:00:00Z",
      },
      operations: [],
    } as never);
    vi.mocked(api.startExecution).mockResolvedValue({
      session: {
        id: "exec-1",
        status: "COMPLETED",
        summary: { applied: 7, preflightOk: 7 },
        rollbackAvailable: true,
        confirmationPhraseRequired: false,
        createdAt: "2026-08-18T10:00:00Z",
      },
      operations: [],
    } as never);
    vi.mocked(api.rollbackExecution).mockResolvedValue({
      session: {
        id: "exec-1",
        status: "ROLLED_BACK",
        summary: { applied: 0 },
        rollbackAvailable: false,
        confirmationPhraseRequired: false,
        createdAt: "2026-08-18T10:00:00Z",
      },
      operations: [],
    } as never);
  });

  it("summarizes consumer destinations without exposing confidence scores", () => {
    const summary = summarizeProposals([proposal]);
    expect(summary.filesToOrganize).toBe(7);
    expect(summary.counts.Documents).toBe(2);
    expect(summary.counts.Images).toBe(1);
    expect(summary.counts.Installateurs).toBe(1);
    expect(summary.counts["À vérifier"]).toBe(1);
  });

  it("shows a three-step first launch then Ranger mon ordinateur", async () => {
    render(<OnboardingView onSelectFolder={vi.fn()} onComplete={vi.fn()} onStartWholeComputer={vi.fn()} />);
    expect(
      screen.getByRole("heading", {
        name: "ZEMO range vos fichiers, pas vos applications.",
      }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(
      screen.getByRole("heading", {
        name: "Vous voyez toujours un aperçu avant le rangement.",
      }),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Continuer" }));
    expect(
      screen.getByRole("heading", {
        name: "Vous pouvez annuler après le rangement.",
      }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Ranger mon ordinateur" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Choisir les dossiers" })).toBeTruthy();
  });

  it("walks home → scan → preview → apply → done → undo", async () => {
    markOnboardingCompleted();
    render(<App />);
    expect(
      await screen.findByRole("heading", { name: "Votre ordinateur est en bazar ?" }),
    ).toBeTruthy();
    expect(
      screen.getByText(
        /range vos fichiers personnels sans toucher à vos applications/i,
      ),
    ).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Ranger mon ordinateur" }));

    await waitFor(() => {
      expect(api.probeUserContentAccess).toHaveBeenCalled();
      expect(api.registerUserContentRoot).toHaveBeenCalled();
      expect(api.scanWorkspace).toHaveBeenCalled();
      expect(api.generateOrganizationProposal).toHaveBeenCalled();
    });
    const generateArgs = vi.mocked(api.generateOrganizationProposal).mock.calls[0];
    expect(generateArgs[3]).toBe(true);

    expect(
      await screen.findByRole("heading", {
        name: /ZEMO peut ranger 7 fichiers/i,
      }),
    ).toBeTruthy();
    expect(screen.getByText("Documents")).toBeTruthy();
    expect(screen.getByText("Installateurs")).toBeTruthy();
    expect(screen.queryByText(/confidence|moteur local|journal sequence/i)).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Appliquer le rangement" }));
    expect(
      await screen.findByRole("heading", { name: "Votre ordinateur est rangé." }),
    ).toBeTruthy();
    expect(screen.getByText(/7 fichiers rangés/i)).toBeTruthy();
    expect(screen.getByText(/0 fichier supprimé/i)).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Annuler le rangement" }));
    await waitFor(() => {
      expect(api.rollbackExecution).toHaveBeenCalledWith("exec-1");
    });
  });

  it("keeps advanced architecture out of the primary nav", async () => {
    markOnboardingCompleted();
    render(<App />);
    await screen.findByRole("heading", { name: "Votre ordinateur est en bazar ?" });
    const nav = screen.getByRole("navigation", { name: "Navigation principale" });
    expect(within(nav).getByRole("button", { name: "Accueil" })).toBeTruthy();
    expect(within(nav).getByRole("button", { name: "Recherche" })).toBeTruthy();
    fireEvent.click(screen.getByText("Options avancées"));
    expect(within(nav).getByRole("button", { name: "Organisation détaillée" })).toBeTruthy();
    expect(within(nav).getByRole("button", { name: "Inventaire" })).toBeTruthy();
    expect(within(nav).getByRole("button", { name: "Historique" })).toBeTruthy();
  });

  it("asks for in-product authorization instead of dumping the user home", async () => {
    markOnboardingCompleted();
    vi.mocked(api.probeUserContentAccess).mockResolvedValue([
      toProbe(locations[0], "authorization_required"),
    ]);
    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Ranger mon ordinateur" }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "ZEMO a besoin de votre autorisation pour accéder à ce dossier.",
      }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Autoriser l’accès" })).toBeTruthy();
    expect(screen.getByText("Bureau — Autorisation nécessaire")).toBeTruthy();
    expect(screen.queryByText(/Aucun dossier n’a pu être analysé/i)).toBeNull();
    expect(screen.queryByText(/EACCES|TCC|System Settings|Full Disk Access/i)).toBeNull();
    expect(api.registerUserContentRoot).not.toHaveBeenCalled();
    expect(api.scanWorkspace).not.toHaveBeenCalled();
  });

  it("keeps Autoriser visible when packaged inspect fails as unexpected_error", async () => {
    markOnboardingCompleted();
    vi.mocked(api.probeUserContentAccess).mockResolvedValue([
      {
        ...toProbe(locations[0], "unexpected_error"),
        humanStatus: "Bureau — Impossible à analyser",
        failedStage: "inspect_volume",
        errorKind: "Other",
        technicalDetails: "Folder: desktop\nStage: inspect_volume\nAccessState: unexpected_error",
      },
    ]);
    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Ranger mon ordinateur" }),
    );
    expect(
      await screen.findByRole("heading", {
        name: "ZEMO a besoin de votre autorisation pour accéder à ce dossier.",
      }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Autoriser l’accès" })).toBeTruthy();
    expect(screen.queryByText(/Aucun dossier n’a pu être analysé/i)).toBeNull();
    fireEvent.click(screen.getByText("Détails techniques"));
    expect(screen.getByText(/inspect_volume/)).toBeTruthy();
  });

  it("scans accessible folders when another folder still needs authorization", async () => {
    markOnboardingCompleted();
    vi.mocked(api.probeUserContentAccess).mockResolvedValue([
      toProbe(locations[0], "accessible"),
      {
        ...toProbe(locations[0], "authorization_required"),
        kind: "documents",
        logicalName: "documents",
        displayLabel: "Documents",
        humanStatus: "Documents — Autorisation nécessaire",
      },
    ]);
    render(<App />);
    fireEvent.click(
      await screen.findByRole("button", { name: "Ranger mon ordinateur" }),
    );
    expect(
      await screen.findByRole("heading", { name: /ZEMO peut ranger 7 fichiers/i }),
    ).toBeTruthy();
    expect(screen.getByText(/1 dossier nécessite votre autorisation/i)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Autoriser" })).toBeTruthy();
    expect(api.registerUserContentRoot).toHaveBeenCalledWith("workspace-1", "desktop");
    expect(api.registerUserContentRoot).not.toHaveBeenCalledWith(
      "workspace-1",
      "documents",
    );
    expect(screen.queryByText(/Aucun dossier n’a pu être analysé/i)).toBeNull();
  });
});
