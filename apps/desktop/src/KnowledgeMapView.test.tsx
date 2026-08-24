// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import type { LocalFileDetail } from "./types";
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
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
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
    });

    render(<KnowledgeMapView workspaceId="workspace-1" onClose={() => undefined} />);

    expect(await screen.findByText("Votre carte apparaîtra ici")).toBeTruthy();
    expect(screen.getByText(/Aucun fichier n’est déplacé/i)).toBeTruthy();
  });

  it("renders real semantic groups and supports selecting an identity", async () => {
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
      query: "",
      page: 0,
      pageSize: 120,
      total: 1,
      hasMore: false,
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
    });

    render(<KnowledgeMapView workspaceId="workspace-1" onClose={() => undefined} />);

    expect(await screen.findByText("Carte ZEMO")).toBeTruthy();
    await waitFor(() => expect(screen.getAllByText("Point P").length).toBeGreaterThan(0));
    expect(screen.getAllByText("Factures").length).toBeGreaterThan(0);

    const identityNode = screen.getByRole("button", {
      name: /Entreprise \/ organisation Point P, 1 fichier/i,
    });
    fireEvent.click(identityNode);

    expect(await screen.findByText("Voir l’identité et les preuves")).toBeTruthy();
  });

  it("filters the map to professional context", async () => {
    vi.mocked(api.searchLocalFiles).mockResolvedValue({
      query: "",
      page: 0,
      pageSize: 120,
      total: 1,
      hasMore: false,
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
    });

    render(<KnowledgeMapView workspaceId="workspace-1" onClose={() => undefined} />);
    await screen.findByText("Carte ZEMO");

    fireEvent.click(screen.getByRole("button", { name: "Pro" }));
    expect(screen.getByRole("button", { name: "Pro" }).className).toContain("is-active");
  });
});
