// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import { FileDetailPanel } from "./FileDetailPanel";
import { IdentityDetailPanel } from "./IdentityDetailPanel";
import { IdentityReviewView } from "./IdentityReviewView";
import type { IdentityDetail, IdentityReviewPage, LocalFileDetail } from "./types";

vi.mock("./api", () => ({
  cancelIdentityResolution: vi.fn(),
  decideIdentityCandidate: vi.fn(),
  getErrorMessage: (error: unknown) => String(error),
  getFileDetail: vi.fn(),
  getIdentityDetail: vi.fn(),
  listIdentityReviewGroups: vi.fn(),
  resolveIdentities: vi.fn(),
  subscribeIdentityResolutionProgress: vi.fn(),
  storeSemanticCorrection: vi.fn(),
  unlinkIdentityOccurrence: vi.fn(),
}));

const reviewPage: IdentityReviewPage = {
  total: 1,
  limit: 30,
  offset: 0,
  hasMore: false,
  items: [
    {
      reviewGroupId: "group-1",
      reviewReason: "POSSIBLE_DUPLICATE_IDENTITY",
      groupKey: "organization:point p",
      title: "Point P ↔ POINT.P SAS",
      explanation: "Rapprochement local à confirmer.",
      maxScore: 0.88,
      candidateCount: 1,
      occurrenceCount: 19,
      fileCount: 19,
      status: "NEEDS_REVIEW",
      resolverVersion: "6.0.0",
      createdAt: "2026-08-10T20:00:00Z",
      updatedAt: "2026-08-10T20:00:00Z",
      candidates: [
        {
          candidateId: "candidate-1",
          reviewGroupKey: "organization:point p",
          score: 0.88,
          policyDecision: "REVIEW",
          status: "CANDIDATE",
          resolverVersion: "6.0.0",
          createdAt: "2026-08-10T20:00:00Z",
          updatedAt: "2026-08-10T20:00:00Z",
          left: {
            identityId: "identity-left",
            identityType: "ORGANIZATION",
            displayName: "Point P",
            normalizedDisplayName: "point p",
            resolutionStatus: "CANDIDATE",
            lifecycleStatus: "ACTIVE",
            confidence: 0.9,
            userLocked: false,
            occurrenceCount: 12,
            fileCount: 12,
            aliases: ["Point P"],
            roles: ["SUPPLIER"],
          },
          right: {
            identityId: "identity-right",
            identityType: "ORGANIZATION",
            displayName: "POINT.P SAS",
            normalizedDisplayName: "point p sas",
            resolutionStatus: "CANDIDATE",
            lifecycleStatus: "ACTIVE",
            confidence: 0.86,
            userLocked: false,
            occurrenceCount: 7,
            fileCount: 7,
            aliases: ["POINT.P SAS"],
            roles: ["SUPPLIER"],
          },
          evidence: [
            {
              evidenceType: "NORMALIZED_NAME",
              strength: "MEDIUM",
              polarity: "SUPPORTS",
              leftValue: "<script>Point P</script>",
              rightValue: "point p",
              weight: 0.54,
              explanation: "Noms normalisés compatibles",
            },
            {
              evidenceType: "COMPANY_IDENTIFIER",
              strength: "CONFLICTING",
              polarity: "CONFLICTS",
              leftValue: "111",
              rightValue: "222",
              weight: 0,
              explanation: "Identifiants différents",
            },
          ],
        },
      ],
    },
  ],
};

const identityDetail: IdentityDetail = {
  identity: {
    identityId: "identity-left",
    identityType: "ORGANIZATION",
    displayName: "Point P",
    normalizedDisplayName: "point p",
    resolutionStatus: "USER_CONFIRMED",
    lifecycleStatus: "ACTIVE",
    confidence: 0.98,
    userLocked: true,
    occurrenceCount: 2,
    fileCount: 2,
    aliases: ["Point P", "POINT.P SAS"],
    roles: ["SUPPLIER"],
  },
  occurrenceTotal: 2,
  occurrencesTruncated: false,
  occurrences: [
    {
      occurrenceId: "occurrence-1",
      fileId: "file-1",
      filename: "invoice.pdf",
      relativePath: "Invoices/invoice.pdf",
      originalValue: "Point P",
      normalizedValue: "point p",
      confidence: 0.94,
      role: "SUPPLIER",
      analyzerVersion: "5.0.0",
      active: true,
    },
    {
      occurrenceId: "occurrence-2",
      fileId: "file-2",
      filename: "quote.pdf",
      relativePath: "Quotes/quote.pdf",
      originalValue: "POINT.P SAS",
      normalizedValue: "point p sas",
      confidence: 0.91,
      role: "SUPPLIER",
      analyzerVersion: "5.0.0",
      active: true,
    },
  ],
  identifiers: [{ kind: "COMPANY_IDENTIFIER", value: "73282932000074" }],
  relationships: [],
  projects: [
    {
      identityId: "project-1",
      identityType: "PROJECT",
      displayName: "Martin Bordeaux",
      normalizedDisplayName: "martin bordeaux",
      resolutionStatus: "CANDIDATE",
      lifecycleStatus: "ACTIVE",
      confidence: 0.86,
      userLocked: false,
      occurrenceCount: 2,
      fileCount: 2,
      aliases: ["Martin Bordeaux"],
      roles: [],
    },
  ],
  auditEvents: [
    {
      eventType: "USER_CONFIRMED",
      decisionSource: "USER",
      reason: "Confirmation locale",
      createdAt: "2026-08-10T20:00:00Z",
    },
  ],
  resolverVersion: "6.0.0",
  updatedAt: "2026-08-10T20:00:00Z",
};

const relatedFileDetail: LocalFileDetail = {
  fileId: "file-1",
  fileVersionId: "version-1",
  filename: "invoice.pdf",
  relativePath: "Invoices/invoice.pdf",
  extension: "pdf",
  detectedType: "application/pdf",
  byteSize: 2_048,
  duplicate: false,
  extractionStatus: "SUCCESS",
  extractorType: "pdf_text",
  extractorVersion: "1.0.0",
  ocrStatus: "NOT_USED",
  textPreview: "Supplier: Point P",
  characterCount: 17,
  reviewItems: [],
  semanticAnalysis: null,
  relationships: [
    {
      relationshipId: "relationship-1",
      relationshipType: "FILE_SUPPLIER",
      identityId: "identity-left",
      displayName: "Point P",
      identityType: "ORGANIZATION",
      confidence: 0.98,
      status: "USER_CONFIRMED",
      userConfirmationState: "CONFIRMED",
      evidence: ["<img src=x onerror=deleteFiles()>"],
    },
  ],
};

describe("Milestone 6 identity review", () => {
  afterEach(cleanup);

  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(api.listIdentityReviewGroups).mockResolvedValue(reviewPage);
    vi.mocked(api.subscribeIdentityResolutionProgress).mockResolvedValue(() => undefined);
    vi.mocked(api.cancelIdentityResolution).mockResolvedValue(true);
    vi.mocked(api.resolveIdentities).mockResolvedValue({
      runId: "run-1",
      workspaceId: "workspace-1",
      triggerKind: "MANUAL",
      status: "COMPLETED",
      resolverId: "deterministic-cross-file-resolution",
      resolverVersion: "6.0.0",
      filesConsidered: 2,
      occurrencesProcessed: 4,
      blockingMemberships: 8,
      comparisons: 2,
      candidatesCreated: 1,
      autoLinksCreated: 0,
      startedAt: "2026-08-10T20:00:00Z",
      completedAt: "2026-08-10T20:00:01Z",
    });
    vi.mocked(api.decideIdentityCandidate).mockResolvedValue({
      decisionId: "decision-1",
      primaryIdentityId: "identity-left",
      secondaryIdentityId: "identity-right",
      action: "CONFIRM_MATCH",
      createdAt: "2026-08-10T20:01:00Z",
    });
    vi.mocked(api.getIdentityDetail).mockResolvedValue(identityDetail);
    vi.mocked(api.getFileDetail).mockResolvedValue(relatedFileDetail);
    vi.mocked(api.unlinkIdentityOccurrence).mockResolvedValue({
      decisionId: "decision-2",
      primaryIdentityId: "identity-left",
      secondaryIdentityId: "identity-split",
      occurrenceId: "occurrence-2",
      action: "UNLINK_OCCURRENCE",
      createdAt: "2026-08-10T20:02:00Z",
    });
  });

  it("shows grouped evidence safely and records confirmation", async () => {
    const { container } = render(
      <IdentityReviewView
        workspaceId="workspace-1"
        onOpenIdentity={() => undefined}
      />,
    );
    expect(await screen.findByText("Point P ↔ POINT.P SAS")).toBeTruthy();
    expect(container.textContent).toContain("<script>Point P</script>");
    expect(container.querySelector("script")).toBeNull();
    expect(screen.getByText("Identifiants différents")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Confirmer identiques" }));
    await waitFor(() => {
      expect(api.decideIdentityCandidate).toHaveBeenCalledWith(
        "candidate-1",
        "confirm",
        expect.any(String),
      );
    });
  });

  it("persists keep-separate decisions from review", async () => {
    render(
      <IdentityReviewView
        workspaceId="workspace-1"
        onOpenIdentity={() => undefined}
      />,
    );
    expect(await screen.findByText("Point P ↔ POINT.P SAS")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Garder séparées" }));
    await waitFor(() => {
      expect(api.decideIdentityCandidate).toHaveBeenCalledWith(
        "candidate-1",
        "keep_separate",
        expect.any(String),
      );
    });
  });

  it("offers cancellation immediately while local resolution is running", async () => {
    let finishResolution: (() => void) | undefined;
    vi.mocked(api.resolveIdentities).mockImplementation(
      () =>
        new Promise((resolve) => {
          finishResolution = () =>
            resolve({
              runId: "run-2",
              workspaceId: "workspace-1",
              triggerKind: "MANUAL",
              status: "CANCELLED",
              resolverId: "deterministic-cross-file-resolution",
              resolverVersion: "6.0.0",
              filesConsidered: 1,
              occurrencesProcessed: 1,
              blockingMemberships: 2,
              comparisons: 0,
              candidatesCreated: 0,
              autoLinksCreated: 0,
              startedAt: "2026-08-10T20:00:00Z",
              completedAt: "2026-08-10T20:00:01Z",
            });
        }),
    );
    render(
      <IdentityReviewView
        workspaceId="workspace-1"
        onOpenIdentity={() => undefined}
      />,
    );
    fireEvent.click(
      await screen.findByRole("button", { name: "Relancer localement" }),
    );
    fireEvent.click(await screen.findByRole("button", { name: "Annuler" }));
    expect(api.cancelIdentityResolution).toHaveBeenCalledWith("workspace-1");
    finishResolution?.();
    expect(
      await screen.findByText(
        "Résolution annulée ; les résultats déjà validés restent cohérents.",
      ),
    ).toBeTruthy();
  });

  it("shows identity provenance and requires a two-step semantic unlink", async () => {
    const onOpenFile = vi.fn();
    const onOpenIdentity = vi.fn();
    render(
      <IdentityDetailPanel
        identityId="identity-left"
        onClose={() => undefined}
        onOpenFile={onOpenFile}
        onOpenIdentity={onOpenIdentity}
      />,
    );
    expect(await screen.findByText("73282932000074")).toBeTruthy();
    expect(screen.getByText("POINT.P SAS")).toBeTruthy();
    const splitButtons = screen.getAllByRole("button", {
      name: "Séparer cette occurrence",
    });
    fireEvent.click(splitButtons[1]);
    const confirmation = screen.getByRole("button", {
      name: "Confirmer la séparation",
    });
    fireEvent.click(confirmation);
    await waitFor(() => {
      expect(api.unlinkIdentityOccurrence).toHaveBeenCalledWith(
        "identity-left",
        "occurrence-2",
        expect.any(String),
      );
    });
    fireEvent.click(
      screen.getByRole("button", { name: "Martin Bordeaux" }),
    );
    expect(onOpenIdentity).toHaveBeenCalledWith("project-1");
  });

  it("exposes safe, clickable relationships from File Detail", async () => {
    const onOpenIdentity = vi.fn();
    const { container } = render(
      <FileDetailPanel
        fileId="file-1"
        onClose={() => undefined}
        onOpenIdentity={onOpenIdentity}
      />,
    );
    expect(await screen.findByText("Point P")).toBeTruthy();
    fireEvent.click(screen.getByText("Pourquoi ?"));
    expect(container.textContent).toContain("<img src=x onerror=deleteFiles()>");
    expect(container.querySelector("img")).toBeNull();
    fireEvent.click(
      screen.getByRole("button", { name: "Voir l’identité et les preuves" }),
    );
    expect(onOpenIdentity).toHaveBeenCalledWith("identity-left");
  });
});
