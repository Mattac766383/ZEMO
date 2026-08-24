// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import * as api from "./api";
import type { LocalFileDetail } from "./types";
import { FileDetailPanel } from "./FileDetailPanel";

vi.mock("./api", () => ({
  getFileDetail: vi.fn(),
  storeSemanticCorrection: vi.fn(),
  getErrorMessage: vi.fn((reason: unknown) => String(reason)),
}));

function baseDetail(): LocalFileDetail {
  return {
    fileId: "file-1",
    fileVersionId: "version-1",
    filename: "Facture_PointP_1482.pdf",
    relativePath: "Documents/Facture_PointP_1482.pdf",
    extension: "pdf",
    detectedType: "pdf",
    byteSize: 148200,
    createdAt: "2026-04-12T10:00:00Z",
    modifiedAt: "2026-04-12T11:00:00Z",
    hash: "abc123",
    duplicate: false,
    extractionStatus: "SUCCESS",
    extractorType: "pdf",
    extractorVersion: "1",
    ocrStatus: "NOT_USED",
    textPreview: "POINT.P BORDEAUX\nTOTAL 1482,40 EUR",
    characterCount: 340,
    reviewItems: [],
    semanticAnalysis: null,
    relationships: [],
  };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("FileDetailPanel V2", () => {
  it("surfaces real semantic understanding before technical details", async () => {
    const detail = baseDetail();
    detail.semanticAnalysis = {
      analysisId: "analysis-1",
      status: "COMPLETED",
      analyzerId: "local",
      analyzerVersion: "semantic-local-test",
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
          fieldId: "type",
          fieldKey: "document_type",
          valueKind: "document_type",
          displayValue: "invoice",
          machineDisplayValue: null,
          normalizedValue: "invoice",
          confidence: 0.97,
          status: "INFERRED",
          sourceMethod: "test",
          analyzerVersion: "semantic-local-test",
          valueSource: "MACHINE",
          userState: null,
          evidence: [],
          candidates: [],
        },
        {
          fieldId: "context",
          fieldKey: "context",
          valueKind: "context",
          displayValue: "business",
          machineDisplayValue: null,
          normalizedValue: "business",
          confidence: 0.96,
          status: "INFERRED",
          sourceMethod: "test",
          analyzerVersion: "semantic-local-test",
          valueSource: "MACHINE",
          userState: null,
          evidence: [],
          candidates: [],
        },
        {
          fieldId: "amount",
          fieldKey: "total",
          valueKind: "money",
          displayValue: "1 482,40",
          machineDisplayValue: null,
          normalizedValue: 148240,
          confidence: 0.95,
          status: "INFERRED",
          sourceMethod: "test",
          analyzerVersion: "semantic-local-test",
          valueSource: "MACHINE",
          userState: null,
          evidence: [
            {
              evidenceType: "text_span",
              exactText: "TOTAL 1482,40 EUR",
              sourceLabel: "PDF page 1",
              explanation: "Montant total détecté",
              extractionMethod: "pdf_text",
              analyzerVersion: "semantic-local-test",
              pageNumber: 1,
            },
          ],
          candidates: [],
        },
        {
          fieldId: "currency",
          fieldKey: "currency",
          valueKind: "currency",
          displayValue: "EUR",
          machineDisplayValue: null,
          normalizedValue: "EUR",
          confidence: 0.99,
          status: "INFERRED",
          sourceMethod: "test",
          analyzerVersion: "semantic-local-test",
          valueSource: "MACHINE",
          userState: null,
          evidence: [],
          candidates: [],
        },
      ],
      entities: [],
    };
    detail.relationships = [
      {
        relationshipId: "relationship-1",
        relationshipType: "FILE_SUPPLIER",
        identityId: "identity-point-p",
        displayName: "Point P",
        identityType: "ORGANIZATION",
        confidence: 0.96,
        status: "CONFIRMED",
        userConfirmationState: null,
        evidence: ["POINT.P BORDEAUX"],
      },
    ];
    vi.mocked(api.getFileDetail).mockResolvedValue(detail);

    render(<FileDetailPanel fileId="file-1" onClose={() => undefined} />);

    expect(
      await screen.findByRole("heading", { name: "Facture_PointP_1482.pdf" }),
    ).toBeTruthy();
    expect(screen.getByText("Ce que ZEMO a compris")).toBeTruthy();
    expect(screen.getAllByText("Point P").length).toBeGreaterThan(0);
    expect(screen.getByText(/Montant détecté : 1 482,40 EUR/i)).toBeTruthy();
    expect(screen.getByText("Informations techniques du fichier")).toBeTruthy();
  });

  it("does not invent a semantic summary when analysis is unavailable", async () => {
    vi.mocked(api.getFileDetail).mockResolvedValue(baseDetail());

    render(<FileDetailPanel fileId="file-1" onClose={() => undefined} />);

    expect(
      await screen.findByText(
        "ZEMO n’a pas encore suffisamment d’informations structurées sur ce fichier.",
      ),
    ).toBeTruthy();
    expect(screen.getByText("PDF")).toBeTruthy();
  });
});
