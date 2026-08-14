// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { OrganizationPreviewView } from "./OrganizationPreviewView";
import type { OrganizationProposal } from "./types";

vi.mock("./api", () => ({
  approveExecution: vi.fn(),
  cancelOrganizationProposal: vi.fn(),
  cancelExecution: vi.fn(),
  generateOrganizationProposal: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
  getExecutionStatus: vi.fn(),
  getLatestOrganizationProposal: vi.fn(),
  getSystemStatus: vi.fn(),
  listExecutionHistory: vi.fn(),
  pauseExecution: vi.fn(),
  prepareExecution: vi.fn(),
  recoverExecution: vi.fn(),
  refreshOrganizationProposalDrift: vi.fn(),
  rollbackExecution: vi.fn(),
  setOrganizationProposalOverride: vi.fn(),
  setOrganizationProposalStatus: vi.fn(),
  startExecution: vi.fn(),
  subscribeExecutionProgress: vi.fn(),
  subscribeOrganizationProposalProgress: vi.fn(),
}));

const proposal: OrganizationProposal = {
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
    filesAnalyzed: 2,
    proposedMoves: 1,
    proposedRenames: 1,
    unchanged: 0,
    needsReview: 1,
    unresolved: 1,
    conflicts: 1,
    duplicateNoAction: 0,
    highConfidence: 1,
    mediumConfidence: 1,
    lowConfidence: 0,
    averageDepth: 3,
    maximumDepth: 3,
  },
  change: {
    destinationsChanged: 0,
    filesAdded: 2,
    conflictsResolved: 0,
    movedToReview: 1,
  },
  operations: [
    {
      id: "operation-1",
      fileId: "file-1",
      fileVersionId: "version-1",
      operationKind: "MOVE_PROPOSAL",
      sourceRelativePath: "Downloads/scan_38492.pdf",
      sourceName: "scan_38492.pdf",
      sourceHash: "abcdef",
      sourceByteSize: 2_048,
      sourceModifiedAt: "2026-08-10T19:00:00Z",
      machineDestination: ["Business", "Clients", "Dupont SARL"],
      machineName: "2026-06-17_Invoice_FP-39482.pdf",
      proposedDestination: ["Business", "Clients", "Dupont SARL"],
      proposedName: "2026-06-17_Invoice_FP-39482.pdf",
      proposedRelativePath:
        "Business/Clients/Dupont SARL/2026-06-17_Invoice_FP-39482.pdf",
      confidenceScore: 0.96,
      confidenceLevel: "VERY_HIGH",
      conflictState: "NONE",
      needsReview: false,
      userOverride: false,
      duplicateGroupId: null,
      duplicateCanonical: true,
      customerName: "Dupont SARL",
      supplierName: "Point P",
      projectName: "Project Bordeaux",
      semanticContext: "BUSINESS",
      documentType: "INVOICE",
      disruptionScore: 0.3,
      proposedDepth: 3,
      proposedPathLength: 67,
      reasons: [
        {
          code: "CUSTOMER",
          explanation: "<script>deleteFiles()</script>",
          evidenceReferences: ["identity:file-1"],
        },
      ],
      stale: false,
    },
    {
      id: "operation-2",
      fileId: "file-2",
      fileVersionId: "version-2",
      operationKind: "TO_REVIEW",
      sourceRelativePath: "Downloads/unknown.bin",
      sourceName: "unknown.bin",
      sourceHash: null,
      sourceByteSize: 128,
      sourceModifiedAt: "2026-08-10T19:00:00Z",
      machineDestination: ["TO_REVIEW"],
      machineName: "unknown.bin",
      proposedDestination: ["TO_REVIEW"],
      proposedName: "unknown.bin",
      proposedRelativePath: "TO_REVIEW/unknown.bin",
      confidenceScore: 0.5,
      confidenceLevel: "MEDIUM",
      conflictState: "UNRESOLVED",
      needsReview: true,
      userOverride: false,
      duplicateGroupId: null,
      duplicateCanonical: true,
      customerName: null,
      supplierName: null,
      projectName: null,
      semanticContext: "UNKNOWN",
      documentType: "UNKNOWN",
      disruptionScore: 0.8,
      proposedDepth: 1,
      proposedPathLength: 21,
      reasons: [],
      stale: false,
    },
  ],
  nodes: [
    {
      id: "node-root",
      parentId: null,
      kind: "ROOT",
      name: "Organization Preview",
      virtualPath: "",
      operationId: null,
      childCount: 2,
      needsReviewCount: 1,
      conflictCount: 1,
    },
    {
      id: "node-business",
      parentId: "node-root",
      kind: "FOLDER",
      name: "Business",
      virtualPath: "Business",
      operationId: null,
      childCount: 1,
      needsReviewCount: 0,
      conflictCount: 0,
    },
    {
      id: "node-file-1",
      parentId: "node-business",
      kind: "FILE",
      name: "2026-06-17_Invoice_FP-39482.pdf",
      virtualPath:
        "Business/Clients/Dupont SARL/2026-06-17_Invoice_FP-39482.pdf",
      operationId: "operation-1",
      childCount: 0,
      needsReviewCount: 0,
      conflictCount: 0,
    },
    {
      id: "node-review",
      parentId: "node-root",
      kind: "FOLDER",
      name: "TO_REVIEW",
      virtualPath: "TO_REVIEW",
      operationId: null,
      childCount: 1,
      needsReviewCount: 1,
      conflictCount: 1,
    },
    {
      id: "node-file-2",
      parentId: "node-review",
      kind: "FILE",
      name: "unknown.bin",
      virtualPath: "TO_REVIEW/unknown.bin",
      operationId: "operation-2",
      childCount: 0,
      needsReviewCount: 1,
      conflictCount: 1,
    },
  ],
};

describe("Milestone 7 virtual organization preview", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.getLatestOrganizationProposal).mockResolvedValue(proposal);
    vi.mocked(api.subscribeOrganizationProposalProgress).mockResolvedValue(
      () => undefined,
    );
    vi.mocked(api.setOrganizationProposalOverride).mockResolvedValue(proposal);
    vi.mocked(api.setOrganizationProposalStatus).mockResolvedValue(proposal);
    vi.mocked(api.refreshOrganizationProposalDrift).mockResolvedValue(proposal);
    vi.mocked(api.cancelOrganizationProposal).mockResolvedValue(true);
    vi.mocked(api.getSystemStatus).mockResolvedValue({
      localFirst: true,
      readOnlyScan: true,
      networkDisabled: true,
      applyEnabled: false,
      applyGateReason: "Native mutation unavailable in this UI regression.",
      displayLabel: "Local test",
      version: "8.0.0",
      recoveryRequired: false,
      journalLocked: false,
      journalDiagnostics: [],
    });
    vi.mocked(api.listExecutionHistory).mockResolvedValue([]);
    vi.mocked(api.subscribeExecutionProgress).mockResolvedValue(() => undefined);
  });

  it("makes simulation safety and original-versus-proposed paths unmistakable", async () => {
    const { container } = render(
      <OrganizationPreviewView workspaceId="workspace-1" />,
    );
    expect(
      await screen.findByRole("heading", { name: "Organisation proposée" }),
    ).toBeTruthy();
    expect(
      screen.getByText("Rien n’a encore été modifié sur votre ordinateur."),
    ).toBeTruthy();
    fireEvent.click(screen.getByText("2026-06-17_Invoice_FP-39482.pdf"));
    expect(screen.getByText("Actuellement")).toBeTruthy();
    expect(screen.getAllByText("Proposition").length).toBeGreaterThan(0);
    expect(screen.getByText("Downloads/scan_38492.pdf")).toBeTruthy();
    expect(
      screen.getByText(
        "Business/Clients/Dupont SARL/2026-06-17_Invoice_FP-39482.pdf",
      ),
    ).toBeTruthy();
    expect(container.textContent).toContain("<script>deleteFiles()</script>");
    expect(container.querySelector("script")).toBeNull();
    expect(screen.queryByRole("button", { name: /^apply$/i })).toBeNull();
  });

  it("filters review cases and saves authoritative virtual edits", async () => {
    render(<OrganizationPreviewView workspaceId="workspace-1" />);
    expect(
      await screen.findByText("2026-06-17_Invoice_FP-39482.pdf"),
    ).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Afficher"), {
      target: { value: "review" },
    });
    expect(screen.getAllByText("unknown.bin").length).toBeGreaterThan(0);
    expect(
      screen.queryByText("2026-06-17_Invoice_FP-39482.pdf"),
    ).toBeNull();

    fireEvent.change(screen.getByLabelText("Afficher"), {
      target: { value: "all" },
    });
    fireEvent.click(screen.getByText("2026-06-17_Invoice_FP-39482.pdf"));
    fireEvent.click(screen.getByText("Modifier"));
    fireEvent.change(screen.getByLabelText("Dossier proposé"), {
      target: { value: "Business\\Chosen by user" },
    });
    fireEvent.change(screen.getByLabelText("Nom proposé"), {
      target: { value: "Chosen_Invoice.pdf" },
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Enregistrer la modification" }),
    );
    await waitFor(() => {
      expect(api.setOrganizationProposalOverride).toHaveBeenCalledWith(
        "proposal-1",
        "file-1",
        "destination_and_rename",
        ["Business", "Chosen by user"],
        "Chosen_Invoice.pdf",
        expect.any(String),
      );
    });
  });

  it("records reviewed proposal approval before the separate execution boundary", async () => {
    render(<OrganizationPreviewView workspaceId="workspace-1" />);
    fireEvent.click(
      await screen.findByRole("button", {
        name: "Valider la proposition",
      }),
    );
    await waitFor(() => {
      expect(api.setOrganizationProposalStatus).toHaveBeenCalledWith(
        "proposal-1",
        "approved_for_future_apply",
      );
    });
  });
});
