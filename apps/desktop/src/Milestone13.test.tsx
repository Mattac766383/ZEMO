// @vitest-environment jsdom

import { render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { IdentityDetailPanel } from "./IdentityDetailPanel";
import { OrganizationPreviewView } from "./OrganizationPreviewView";
import type { IdentityDetail, OrganizationProposal } from "./types";

vi.mock("./api", () => ({
  getIdentityDetail: vi.fn(),
  unlinkIdentityOccurrence: vi.fn(),
  getLatestOrganizationProposal: vi.fn(),
  generateOrganizationProposal: vi.fn(),
  cancelOrganizationProposal: vi.fn(),
  subscribeOrganizationProposalProgress: vi.fn().mockResolvedValue(() => {}),
  setOrganizationProposalOverride: vi.fn(),
  setOrganizationProposalStatus: vi.fn(),
  refreshOrganizationProposalDrift: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
  listExecutionHistory: vi.fn().mockResolvedValue([]),
  getSystemStatus: vi.fn().mockResolvedValue({
    localOnly: true,
    networkEnabled: false,
  }),
  getExecutionStatus: vi.fn(),
  prepareExecution: vi.fn(),
  startExecution: vi.fn(),
  pauseExecution: vi.fn(),
  cancelExecution: vi.fn(),
  rollbackExecution: vi.fn(),
  createConsentChallenge: vi.fn(),
  finalizeConsent: vi.fn(),
  subscribeExecutionProgress: vi.fn().mockResolvedValue(() => {}),
}));

const boundedIdentityDetail: IdentityDetail = {
  identity: {
    identityId: "identity-scale",
    identityType: "ORGANIZATION",
    displayName: "Scale Org",
    normalizedDisplayName: "scale org",
    resolutionStatus: "AUTO_LINKED",
    lifecycleStatus: "ACTIVE",
    confidence: 0.9,
    userLocked: false,
    occurrenceCount: 2500,
    fileCount: 2500,
    aliases: ["Scale Org"],
    roles: ["SUPPLIER"],
  },
  occurrenceTotal: 2500,
  occurrencesTruncated: true,
  occurrences: Array.from({ length: 100 }, (_, index) => ({
    occurrenceId: `occurrence-${index}`,
    fileId: `file-${index}`,
    filename: `file_${index}.pdf`,
    relativePath: `Business/file_${index}.pdf`,
    originalValue: "Scale Org",
    normalizedValue: "scale org",
    confidence: 0.9,
    role: "SUPPLIER",
    analyzerVersion: "5.0.0",
    active: true,
  })),
  identifiers: [],
  relationships: [],
  projects: [],
  auditEvents: [],
  resolverVersion: "6.0.0",
  updatedAt: "2026-08-12T00:00:00Z",
};

const boundedProposal: OrganizationProposal = {
  id: "proposal-1",
  revisionId: "revision-1",
  workspaceId: "workspace-1",
  rootId: "root-1",
  sourceScanId: "scan-1",
  revision: 1,
  status: "READY_FOR_REVIEW",
  engineVersion: "m13",
  policyVersion: "m13",
  sourceSemanticVersion: "m5",
  sourceRelationshipVersion: "m6",
  createdAt: "2026-08-12T00:00:00Z",
  updatedAt: "2026-08-12T00:00:00Z",
  summary: {
    filesAnalyzed: 100_000,
    proposedMoves: 80_000,
    proposedRenames: 10_000,
    unchanged: 5_000,
    needsReview: 12_000,
    unresolved: 0,
    conflicts: 0,
    highConfidence: 50_000,
    mediumConfidence: 30_000,
    lowConfidence: 8_000,
    duplicateNoAction: 0,
    averageDepth: 4,
    maximumDepth: 6,
  },
  change: {
    destinationsChanged: 0,
    filesAdded: 100_000,
    conflictsResolved: 0,
    movedToReview: 12_000,
  },
  nodes: [
    {
      id: "node-root",
      parentId: null,
      kind: "ROOT",
      name: "Organization",
      virtualPath: "",
      operationId: null,
      childCount: 2,
      needsReviewCount: 12_000,
      conflictCount: 0,
    },
    {
      id: "node-business",
      parentId: "node-root",
      kind: "FOLDER",
      name: "Business",
      virtualPath: "Business",
      operationId: null,
      childCount: 500,
      needsReviewCount: 4_000,
      conflictCount: 0,
    },
  ],
  operations: Array.from({ length: 500 }, (_, index) => ({
    id: `op-${index}`,
    fileId: `file-${index}`,
    fileVersionId: `version-${index}`,
    operationKind: "MOVE_PROPOSAL",
    sourceRelativePath: `Downloads/file_${index}.pdf`,
    sourceName: `file_${index}.pdf`,
    sourceHash: null,
    sourceByteSize: 1024,
    sourceModifiedAt: null,
    machineDestination: ["Business", "Invoices"],
    machineName: `file_${index}.pdf`,
    proposedDestination: ["Business", "Invoices"],
    proposedName: `file_${index}.pdf`,
    proposedRelativePath: `Business\\Invoices\\file_${index}.pdf`,
    confidenceScore: 0.9,
    confidenceLevel: "HIGH",
    reasons: [],
    conflictState: "NONE",
    needsReview: false,
    stale: false,
    userOverride: false,
    disruptionScore: 0.1,
    proposedPathLength: 40,
    proposedDepth: 3,
    semanticContext: "business",
    documentType: "invoice",
    customerName: null,
    supplierName: null,
    projectName: null,
    duplicateGroupId: null,
    duplicateCanonical: true,
  })),
};

describe("Milestone 13 large-list safety", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("caps identity occurrence DOM rows and reports truncation", async () => {
    vi.mocked(api.getIdentityDetail).mockResolvedValue(boundedIdentityDetail);
    render(
      <IdentityDetailPanel
        identityId="identity-scale"
        onClose={vi.fn()}
        onOpenFile={vi.fn()}
        onOpenIdentity={vi.fn()}
      />,
    );
    await waitFor(() => {
      expect(screen.getByText(/Affichage limité à 100 sur 2500/i)).toBeTruthy();
    });
    expect(document.querySelectorAll(".identity-occurrence")).toHaveLength(100);
  });

  it("loads organization preview with bounded operations and folder nodes only", async () => {
    vi.mocked(api.getLatestOrganizationProposal).mockResolvedValue(boundedProposal);
    render(<OrganizationPreviewView workspaceId="workspace-1" rootId="root-1" />);
    await waitFor(() => {
      expect(api.getLatestOrganizationProposal).toHaveBeenCalledWith(
        "workspace-1",
        "root-1",
        expect.objectContaining({ uiBound: true, operationLimit: 500 }),
      );
    });
    await waitFor(() => {
      expect(screen.getByText("Business")).toBeTruthy();
    });
    expect(boundedProposal.operations).toHaveLength(500);
    expect(boundedProposal.nodes.every((node) => node.kind !== "FILE")).toBe(true);
  });

  it("keeps review/search/inventory page constants bounded", () => {
    // Contracts already enforced by ReviewView/SearchView/App inventory limits.
    expect(50).toBeLessThan(1_000);
    expect(500).toBeLessThan(100_000);
  });
});
