// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import type { LocalFileDetail, LocalSearchPage } from "./types";
import { KnowledgeMapView } from "./KnowledgeMapView";

vi.mock("./api", () => ({
  searchLocalFiles: vi.fn(),
  getFileDetail: vi.fn(),
  probeUserContentAccess: vi.fn(),
  getErrorMessage: vi.fn((reason: unknown) => String(reason)),
  storeSemanticCorrection: vi.fn(),
  getIdentityDetail: vi.fn(),
  unlinkIdentityOccurrence: vi.fn(),
}));

const emptyPage: LocalSearchPage = {
  query: "",
  page: 0,
  pageSize: 120,
  total: 0,
  hasMore: false,
  results: [],
  interpretedQuery: [],
  embeddings: {
    availability: "unavailable",
    providerId: "none",
    version: "none",
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
};

const detail: LocalFileDetail = {
  fileId: "file-1",
  fileVersionId: "version-1",
  filename: "Facture_PointP.pdf",
  relativePath: "Documents/Facture_PointP.pdf",
  extension: "pdf",
  detectedType: "pdf",
  byteSize: 145000,
  createdAt: "2026-04-12T10:00:00Z",
  modifiedAt: "2026-04-12T11:00:00Z",
  hash: "abc123",
  duplicate: false,
  extractionStatus: "SUCCESS",
  extractorType: "pdf",
  extractorVersion: "1",
  ocrStatus: "NOT_USED",
  textPreview: "Facture Point P",
  characterCount: 120,
  reviewItems: [],
  semanticAnalysis: {
    analysisId: "analysis-1",
    status: "COMPLETED",
    analyzerId: "local",
    analyzerVersion: "test-1",
    providerId: "deterministic",
    providerVersion: "1",
    schemaVersion: 1,
    inputQuality: 1,
    inputQualityStatus: "GOOD",
    inputQualityReasons: [],
    language: "fr",
    analyzedAt: "2026-04-12T11:01:00Z",
    fields: [
      {
        fieldId: "field-context",
        fieldKey: "context",
        valueKind: "context",
        displayValue: "business",
        machineDisplayValue: null,
        normalizedValue: "business",
        confidence: 0.97,
        status: "INFERRED",
        sourceMethod: "test",
        analyzerVersion: "test-1",
        valueSource: "MACHINE",
        userState: null,
        evidence: [],
        candidates: [],
      },
      {
        fieldId: "field-type",
        fieldKey: "document_type",
        valueKind: "document_type",
        displayValue: "invoice",
        machineDisplayValue: null,
        normalizedValue: "invoice",
        confidence: 0.96,
        status: "INFERRED",
        sourceMethod: "test",
        analyzerVersion: "test-1",
        valueSource: "MACHINE",
        userState: null,
        evidence: [],
        candidates: [],
      },
    ],
    entities: [],
  },
  relationships: [
    {
      relationshipId: "relationship-1",
      relationshipType: "FILE_SUPPLIER",
      identityId: "identity-point-p",
      displayName: "Point P",
      identityType: "ORGANIZATION",
      confidence: 0.96,
      status: "CONFIRMED",
      userConfirmationState: null,
      evidence: ["Point P présent dans le document"],
    },
  ],
};

function pageWithOneFile(): LocalSearchPage {
  return {
    ...emptyPage,
    total: 1,
    results: [
      {
        fileId: "file-1",
        filename: "Facture_PointP.pdf",
        relativePath: "Documents/Facture_PointP.pdf",
        detectedType: "pdf",
        extension: "pdf",
        byteSize: 145000,
        modifiedAt: "2026-04-12T11:00:00Z",
        extractionStatus: "success",
        ocrStatus: "not_used",
        duplicate: false,
        matchSource: "metadata",
        relevance: 1,
        snippet: "",
        whyMatched: [],
      },
    ],
  };
}

describe("KnowledgeMapView", () => {
  beforeEach(() => {
    vi.mocked(api.probeUserContentAccess).mockResolvedValue([]);
    vi.mocked(api.getFileDetail).mockResolvedValue(detail);
  });

  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it("shows a useful empty state when no indexed file exists", async () => {
    vi.mocked(api.searchLocalFiles).mockResolvedValue(emptyPage);

    render(<KnowledgeMapView workspaceId="workspace-1" onClose={() => undefined} />);

    expect(await screen.findByText("Votre carte apparaîtra ici")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Retour pour analyser mes fichiers/i })).toBeTruthy();
  });

  it("renders semantic groups, search and professional filtering", async () => {
    vi.mocked(api.searchLocalFiles).mockResolvedValue(pageWithOneFile());

    render(<KnowledgeMapView workspaceId="workspace-1" onClose={() => undefined} />);

    await waitFor(() => expect(screen.getAllByText("Point P").length).toBeGreaterThan(0));
    expect(screen.getAllByText("Factures").length).toBeGreaterThan(0);
    expect(screen.getByLabelText("Rechercher dans la carte")).toBeTruthy();

    const professional = screen.getByRole("button", { name: "Pro" });
    fireEvent.click(professional);
    expect(professional.className).toContain("is-active");
  });
});
