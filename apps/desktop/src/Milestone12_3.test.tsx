// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import * as api from "./api";
import {
  classifyUserError,
  shouldShowGlobalBanner,
} from "./errors";
import { resolvePrimaryAction } from "./HomeDashboard";
import { OrganizationPreviewView } from "./OrganizationPreviewView";
import { markOnboardingCompleted, resetOnboardingCompleted } from "./onboardingStorage";
import type { OrganizationProposal } from "./types";

vi.mock("./api", async () => {
  const actual = await vi.importActual<typeof import("./api")>("./api");
  return {
    ...actual,
    getSystemStatus: vi.fn(),
    restoreWorkspaceSession: vi.fn(),
    subscribeScanProgress: vi.fn(async () => () => undefined),
    subscribeContentAnalysisProgress: vi.fn(async () => () => undefined),
    subscribeSemanticAnalysisProgress: vi.fn(async () => () => undefined),
    subscribeIdentityResolutionProgress: vi.fn(async () => () => undefined),
    subscribeOrganizationProposalProgress: vi.fn(async () => () => undefined),
    getMonitoringDashboard: vi.fn(),
    getEmbeddingModelStatus: vi.fn(),
    getLatestOrganizationProposal: vi.fn(),
    getRulesPreferences: vi.fn(),
    searchLocalFiles: vi.fn(),
    listUserContentLocations: vi.fn(),
    selectAndRegisterRoot: vi.fn(),
    registerUserContentRoot: vi.fn(),
    scanWorkspace: vi.fn(),
    prepareExecution: vi.fn(),
    listExecutionHistory: vi.fn(async () => []),
    subscribeExecutionProgress: vi.fn(async () => () => undefined),
    setOrganizationProposalStatus: vi.fn(),
    approveExecution: vi.fn(),
    startExecution: vi.fn(),
    rollbackExecution: vi.fn(),
    recoverExecution: vi.fn(),
    cancelExecution: vi.fn(),
    pauseExecution: vi.fn(),
    getExecutionStatus: vi.fn(),
  };
});

const systemStatus = {
  localFirst: true,
  readOnlyScan: true,
  networkDisabled: true,
  applyEnabled: false,
  applyGateReason: "macOS propose-only",
  recoveryRequired: false,
  journalLocked: false,
  journalDiagnostics: [],
  version: "0.1.0",
};

const proposalFixture = {
  id: "proposal-1",
  revisionId: "revision-1",
  workspaceId: "workspace-1",
  rootId: "root-1",
  sourceScanId: "scan-1",
  revision: 1,
  status: "READY_FOR_REVIEW",
  engineVersion: "7.0.0",
  policyVersion: "7.0.0",
  sourceSemanticVersion: "5.0.0",
  sourceRelationshipVersion: "6.0.0",
  createdAt: "2026-08-10T20:00:00Z",
  updatedAt: "2026-08-10T20:00:01Z",
  summary: {
    filesAnalyzed: 3,
    proposedMoves: 2,
    proposedRenames: 0,
    unchanged: 1,
    needsReview: 0,
    unresolved: 0,
    conflicts: 0,
    duplicateNoAction: 0,
    highConfidence: 2,
    mediumConfidence: 0,
    lowConfidence: 0,
    averageDepth: 3,
    maximumDepth: 4,
  },
  change: {
    destinationsChanged: 0,
    filesAdded: 3,
    conflictsResolved: 0,
    movedToReview: 0,
  },
  operations: [
    {
      id: "op-1",
      fileId: "file-1",
      fileVersionId: "v1",
      operationKind: "MOVE_PROPOSAL",
      sourceRelativePath: "Téléchargements/Facture_45891.pdf",
      sourceName: "Facture_45891.pdf",
      sourceHash: "abc",
      sourceByteSize: 2048,
      sourceModifiedAt: "2026-08-10T19:00:00Z",
      machineDestination: ["Entreprise", "Fournisseurs", "Point P", "Factures", "2026"],
      machineName: "Facture_45891.pdf",
      proposedDestination: ["Entreprise", "Fournisseurs", "Point P", "Factures", "2026"],
      proposedName: "Facture_45891.pdf",
      proposedRelativePath:
        "Entreprise/Fournisseurs/Point P/Factures/2026/Facture_45891.pdf",
      confidenceLevel: "HIGH",
      confidenceScore: 0.97,
      needsReview: false,
      conflictState: "NONE",
      semanticContext: "invoice",
      documentType: "invoice",
      customerName: null,
      projectName: null,
      supplierName: "Point P",
      reasons: [
        {
          code: "supplier_invoice",
          explanation: "Facture Point P détectée • document de 2026",
          evidenceReferences: [],
        },
      ],
    },
  ],
  nodes: [
    {
      id: "root-node",
      kind: "ROOT",
      name: "Proposition",
      path: "",
      parentId: null,
      fileCount: 1,
    },
  ],
} as unknown as OrganizationProposal;

describe("Milestone 12.3 radical simplification + error recovery", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    resetOnboardingCompleted();
    markOnboardingCompleted();
    vi.mocked(api.getSystemStatus).mockResolvedValue(systemStatus);
    vi.mocked(api.restoreWorkspaceSession).mockResolvedValue(null);
    vi.mocked(api.getMonitoringDashboard).mockResolvedValue(null as never);
    vi.mocked(api.getEmbeddingModelStatus).mockResolvedValue({
      status: "not_installed",
      modelId: "local",
      approximateDiskBytes: 118_000_000,
      downloadImplemented: false,
      lastError: null,
      providerId: "none",
      version: "0",
      productionReady: false,
    } as never);
    vi.mocked(api.getLatestOrganizationProposal).mockResolvedValue(null as never);
    vi.mocked(api.getRulesPreferences).mockResolvedValue({
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
        businessRootName: "Entreprise",
        renameTemplate: "{date}_{party}",
        reviewThreshold: 0.65,
      },
    });
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
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
        annIndexStatus: "not_available",
      },
      timings: {
        totalMs: 0,
        lexicalAndStructuredMs: 0,
        queryEmbedMs: 0,
        annMs: 0,
      },
    } as never);
  });

  it("keeps primary navigation to Accueil / Organisation / Recherche / Surveillance", async () => {
    render(<App />);
    const nav = await screen.findByRole("navigation", {
      name: "Navigation principale",
    });
    expect(within(nav).getByRole("button", { name: "Accueil" })).toBeTruthy();
    expect(within(nav).getByRole("button", { name: "Organisation" })).toBeTruthy();
    expect(within(nav).getByRole("button", { name: "Recherche" })).toBeTruthy();
    expect(within(nav).getByRole("button", { name: "Surveillance" })).toBeTruthy();
    expect(within(nav).queryByRole("button", { name: "Exécution" })).toBeNull();
    expect(screen.getByText("Options avancées")).toBeTruthy();

    fireEvent.click(screen.getByText("Options avancées"));
    expect(screen.getByRole("button", { name: "À revoir" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Préférences de rangement" }),
    ).toBeTruthy();
    expect(screen.getByRole("button", { name: "Inventaire" })).toBeTruthy();
    expect(
      screen.getByRole("button", { name: "Historique d’exécution" }),
    ).toBeTruthy();
  });

  it("classifies recoverable engine failures as non-global warnings", () => {
    const classified = classifyUserError(
      "Le moteur local a rencontré une erreur.",
      "search",
    );
    expect(classified.title).not.toMatch(/Problème rencontré/i);
    expect(classified.message).not.toMatch(/moteur local/i);
    expect(classified.severity).toBe("warning");
    expect(classified.scope).toBe("search");
    expect(shouldShowGlobalBanner(classified)).toBe(false);

    const semantic = classifyUserError("embedding provider unavailable", "semantic");
    expect(semantic.scope).toBe("semantic");
    expect(semantic.message).toMatch(/recherche intelligente/i);
    expect(shouldShowGlobalBanner(semantic)).toBe(false);

    const recovery = classifyUserError("recovery journal corrupt", "global");
    expect(recovery.severity).toBe("critical");
    expect(shouldShowGlobalBanner(recovery)).toBe(true);
  });

  it("uses Organiser mon ordinateur / Voir l’organisation proposée as primary CTAs", () => {
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
        root: {
          id: "r",
          displayLabel: "Docs",
          selectedPath: "/tmp",
        },
        scan: {
          id: "s",
          status: "COMPLETED",
          filesDiscovered: 12,
          filesIndexed: 12,
          directoriesDiscovered: 1,
          bytesDiscovered: 1,
          filesHashed: 12,
          duplicateGroups: 0,
          errors: 0,
          skippedItems: 0,
          truncated: false,
        },
        dashboard: null,
        contentNeedsReview: null,
      }).label,
    ).toBe("Voir l’organisation proposée");
  });

  it("shows a user-level Apply path without raw Rust preflight wording", async () => {
    vi.mocked(api.getLatestOrganizationProposal).mockResolvedValue(proposalFixture);
    vi.mocked(api.getSystemStatus).mockResolvedValue({
      ...systemStatus,
      applyEnabled: true,
      applyGateReason: "Apply is available only through the approved execution service.",
    });

    render(<OrganizationPreviewView workspaceId="workspace-1" rootId="root-1" />);

    expect((await screen.findAllByText(/Organisation prête/i)).length).toBeGreaterThan(0);
    expect(
      screen.getAllByText(/Vous pourrez annuler les changements depuis l’historique/i)
        .length,
    ).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Appliquer l’organisation" })).toBeTruthy();
    expect(screen.queryByText(/Final live preflight required/i)).toBeNull();
    expect(screen.queryByText(/Rust will freeze/i)).toBeNull();
    expect(screen.queryByText(/Prepare approved organization/i)).toBeNull();
    fireEvent.click(screen.getByText("Facture_45891.pdf"));
    expect(screen.getByText("Actuellement")).toBeTruthy();
    expect(screen.getAllByText("Proposition").length).toBeGreaterThan(0);
    expect(screen.queryByText(/97\s*%/)).toBeNull();
    expect(screen.queryByText(/Milestone/i)).toBeNull();
    expect(screen.queryByText(/Mutation boundary/i)).toBeNull();
    expect(screen.queryByText(/moteur local/i)).toBeNull();

    fireEvent.click(screen.getByText("Détails techniques"));
    expect(await screen.findByText(/Aucun historique pour le moment/i)).toBeTruthy();
  });

  it("does not show a catastrophic global banner for recoverable search failure", async () => {
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
      safeReadOnly: true,
      filesystemExecutionResumed: false,
    });
    vi.mocked(api.searchLocalFiles).mockRejectedValue(
      new Error("Le moteur local a rencontré une erreur."),
    );

    render(<App />);
    const nav = await screen.findByRole("navigation", {
      name: "Navigation principale",
    });
    fireEvent.click(within(nav).getByRole("button", { name: "Recherche" }));

    expect(
      await screen.findByText(
        /Impossible d’effectuer cette recherche|temporairement indisponible|n’a pas pu être terminée/i,
      ),
    ).toBeTruthy();
    expect(screen.queryByText(/Problème rencontré/i)).toBeNull();
    expect(screen.queryByText(/Le moteur local a rencontré une erreur/i)).toBeNull();
  });

  it("keeps French product copy free of development milestone language on Home", async () => {
    render(<App />);
    expect(
      await screen.findByRole("heading", {
        name: "Organisez et retrouvez vos fichiers.",
      }),
    ).toBeTruthy();
    expect(screen.getByText(/Bêta privée macOS/i)).toBeTruthy();
    expect(screen.queryByText(/Working Name/i)).toBeNull();
    expect(screen.queryByText(/Milestone/i)).toBeNull();
    expect(screen.queryByText(/embedding|ANN|ONNX|IPC/i)).toBeNull();
    expect(
      screen.getByRole("button", { name: "Organiser mon ordinateur" }),
    ).toBeTruthy();
  });
});
