// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { OneClickPreviewView } from "./OneClickOrganize";
import { summarizeProposals } from "./oneClickSummary";
import type { OrganizationProposal } from "./types";

function proposal(): OrganizationProposal {
  const operation = (
    id: string,
    name: string,
    destination: string[],
  ) => ({
    id,
    fileId: `file-${id}`,
    fileVersionId: `version-${id}`,
    sourceRelativePath: name,
    sourceName: name,
    sourceByteSize: 10,
    machineDestination: destination,
    machineName: name,
    proposedDestination: destination,
    proposedName: name,
    proposedRelativePath: [...destination, name].join("/"),
    operationKind: "MOVE_PROPOSAL" as const,
    confidenceScore: 0.95,
    confidenceLevel: "HIGH" as const,
    reasons: [],
    conflictState: "NONE" as const,
    needsReview: false,
    stale: false,
    userOverride: false,
    disruptionScore: 0.1,
    proposedPathLength: 40,
    proposedDepth: destination.length,
    semanticContext: "business",
    documentType: "invoice",
    duplicateCanonical: true,
  });

  return {
    id: "proposal-tree",
    revisionId: "revision-tree",
    workspaceId: "workspace-tree",
    rootId: "root-tree",
    sourceScanId: "scan-tree",
    revision: 1,
    status: "READY_FOR_REVIEW",
    engineVersion: "test",
    policyVersion: "test",
    createdAt: "2026-08-29T00:00:00Z",
    updatedAt: "2026-08-29T00:00:00Z",
    summary: {
      filesAnalyzed: 3,
      proposedMoves: 3,
      proposedRenames: 0,
      unchanged: 0,
      needsReview: 0,
      unresolved: 0,
      conflicts: 0,
      highConfidence: 3,
      mediumConfidence: 0,
      lowConfidence: 0,
      duplicateNoAction: 0,
      averageDepth: 4,
      maximumDepth: 5,
    },
    change: {
      destinationsChanged: 3,
      filesAdded: 3,
      conflictsResolved: 0,
      movedToReview: 0,
    },
    nodes: [],
    operations: [
      operation("1", "invoice-a.pdf", [
        "Professionnel",
        "Clients",
        "Martin",
        "Chantier Bordeaux",
        "Factures",
      ]),
      operation("2", "invoice-b.pdf", [
        "Professionnel",
        "Clients",
        "Martin",
        "Chantier Bordeaux",
        "Factures",
      ]),
      operation("3", "photo.jpg", ["Personnel", "Photos", "2026"]),
    ],
  } as OrganizationProposal;
}

afterEach(cleanup);

describe("simple recursive organization preview", () => {
  it("builds the complete destination hierarchy without filenames", () => {
    const summary = summarizeProposals([proposal()]);
    const professional = summary.counts.folderTree?.find(
      (node) => node.name === "Professionnel",
    );
    expect(professional?.count).toBe(2);
    expect(professional?.children[0]?.name).toBe("Clients");
    expect(professional?.children[0]?.children[0]?.name).toBe("Martin");
    expect(
      professional?.children[0]?.children[0]?.children[0]?.children[0]?.name,
    ).toBe("Factures");
    expect(JSON.stringify(summary.counts.folderTree)).not.toContain("invoice-a.pdf");
  });

  it("reveals subfolders progressively and never exposes file names", () => {
    const summary = summarizeProposals([proposal()]);
    render(
      <OneClickPreviewView
        filesToOrganize={summary.filesToOrganize}
        counts={summary.counts}
        applyBusy={false}
        applyEnabled
        onApply={vi.fn()}
        onSeeDetails={vi.fn()}
      />,
    );

    expect(screen.getByText("Professionnel")).toBeTruthy();
    expect(screen.queryByText("Clients")).toBeNull();
    expect(screen.queryByText("invoice-a.pdf")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /Professionnel/ }));
    const clients = screen.getByRole("button", { name: /Clients/ });
    expect(clients).toBeTruthy();
    fireEvent.click(clients);

    const martin = screen.getByRole("button", { name: /Martin/ });
    fireEvent.click(martin);
    const project = screen.getByRole("button", { name: /Chantier Bordeaux/ });
    fireEvent.click(project);

    const subtree = screen.getByRole("list", {
      name: "Sous-dossiers de Chantier Bordeaux",
    });
    expect(within(subtree).getByText("Factures")).toBeTruthy();
    expect(screen.queryByText("invoice-a.pdf")).toBeNull();
  });
});
